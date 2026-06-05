//! Request dispatch: maps a decoded request frame to a response frame.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use auradb::core::CollectionSchema;
use auradb::query::{FindQuery, Mutation, QueryResultPage, ReadRequest};
use auradb::{Engine, Transaction};
use auradb_core::{Error, Result, ServerCapabilities};
use auradb_observability::Metrics;
use auradb_protocol::{
    AuthRequest, AuthResult, CursorCloseRequest, CursorFetchRequest, ErrorPayload, Frame,
    HealthReport, HealthStatus, HelloAck, HelloRequest, MvccHealth, Opcode, PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::cursor::CursorRegistry;

/// Shared, immutable-ish server context handed to every connection.
#[derive(Clone)]
pub struct ServerContext {
    /// The engine.
    pub engine: Engine,
    /// Metrics registry.
    pub metrics: Arc<Metrics>,
    /// Cursor registry.
    pub cursors: Arc<CursorRegistry>,
    /// Server configuration.
    pub config: Arc<Config>,
    /// Monotonic source of per-connection ids, recorded against transactions
    /// for observability and connection-scoped cleanup.
    pub connection_counter: Arc<std::sync::atomic::AtomicU64>,
    /// The single-node cluster coordinator, present only when cluster mode is
    /// enabled. Drives the Raft write path and reports cluster status.
    pub cluster: Option<Arc<auradb_replication::ClusterNode>>,
}

impl ServerContext {
    /// Mirror the live cluster/Raft state into the metrics registry so an
    /// exported snapshot reflects current consensus state. A no-op when cluster
    /// mode is disabled.
    pub fn refresh_cluster_metrics(&self) {
        let Some(node) = self.cluster.as_ref() else {
            return;
        };
        let status = node.status();
        let m = node.metrics();
        let role_code = match status.role {
            auradb_cluster::NodeRole::Follower => 0,
            auradb_cluster::NodeRole::Candidate => 1,
            auradb_cluster::NodeRole::Leader => 2,
        };
        self.metrics.set_cluster(
            status.enabled,
            role_code,
            status.term,
            status.commit_index,
            status.applied_index,
            status.last_log_index,
            status.replication_lag_entries(),
            m.leader_changes,
            m.votes_granted,
            m.append_entries_sent,
            m.append_entries_received,
            m.apply_errors,
        );
    }
}

/// Build the cluster health summary for a health/status report, or `None` when
/// cluster mode is disabled.
pub fn cluster_health(ctx: &ServerContext) -> Option<auradb_protocol::ClusterHealth> {
    let node = ctx.cluster.as_ref()?;
    let status = node.status();
    Some(auradb_protocol::ClusterHealth {
        enabled: status.enabled,
        node_id: status.node_id.map(|id| id.to_string()),
        cluster_id: status.cluster_id.map(|id| id.to_string()),
        role: status.role.to_string(),
        term: status.term,
        leader_id: status.leader_id.map(|id| id.to_string()),
        commit_index: status.commit_index,
        applied_index: status.applied_index,
        last_log_index: status.last_log_index,
        peer_count: status.peer_count,
        single_node: status.single_node,
        replication_lag_entries: status.replication_lag_entries(),
    })
}

/// Per-connection mutable state.
#[derive(Default)]
pub struct Session {
    /// The connection id, assigned when the connection is accepted.
    pub connection_id: u64,
    /// Negotiated protocol version (0 until HELLO).
    pub negotiated_version: u8,
    /// Whether this connection has authenticated (always true when auth is off).
    pub authenticated: bool,
    /// Open transactions keyed by transaction id.
    pub transactions: HashMap<u64, Transaction>,
    /// Cursor ids owned by this connection (for cleanup on disconnect).
    pub cursor_ids: HashSet<u64>,
}

impl Session {
    /// Clean up resources owned by this session (called on disconnect).
    ///
    /// Open transactions are rolled back through the engine rather than merely
    /// dropped, so each one's pinned MVCC snapshot is released; otherwise an
    /// abandoned transaction would hold its read timestamp forever and stall
    /// version garbage collection.
    pub fn cleanup(&mut self, ctx: &ServerContext) {
        for id in self.cursor_ids.drain() {
            ctx.cursors.close(id);
        }
        for (_id, txn) in self.transactions.drain() {
            ctx.engine.rollback(txn);
        }
        Metrics::gauge_set(&ctx.metrics.active_cursors, ctx.cursors.len() as u64);
        Metrics::gauge_set(
            &ctx.metrics.active_transactions,
            self.transactions.len() as u64,
        );
    }
}

#[derive(Deserialize)]
struct NamePayload {
    name: String,
}

#[derive(Serialize)]
struct TxnBeginResult {
    txn_id: u64,
}

#[derive(Serialize)]
struct OkResult {
    ok: bool,
}

fn ok_ok(frame_req: &Frame) -> Result<Frame> {
    Frame::json(
        Opcode::Ok,
        frame_req.request_id,
        frame_req.txn_id,
        &OkResult { ok: true },
    )
}

/// Handle a single request frame, producing a response frame. Errors are
/// converted to structured `ERROR` frames by [`respond`].
pub fn respond(ctx: &ServerContext, session: &mut Session, frame: Frame) -> Frame {
    Metrics::incr(&ctx.metrics.requests_total);
    match handle(ctx, session, &frame) {
        Ok(response) => response,
        Err(err) => {
            Metrics::incr(&ctx.metrics.errors_total);
            match err.code() {
                auradb_core::ErrorCode::Conflict => {
                    Metrics::incr(&ctx.metrics.mvcc_conflicts_total)
                }
                auradb_core::ErrorCode::TransactionTimeout => {
                    Metrics::incr(&ctx.metrics.mvcc_transaction_timeouts_total)
                }
                _ => {}
            }
            tracing::warn!(request_id = frame.request_id.0, code = %err.code(), error = %err, "request failed");
            ErrorPayload::from_error(&err).to_frame(frame.request_id, frame.txn_id)
        }
    }
}

/// Whether an opcode is gated behind authentication. The handshake, auth,
/// liveness, and readiness probes are always permitted so a client can connect,
/// authenticate, and be health-checked; every data, schema, cursor, explain,
/// migration, and transaction operation requires an authenticated session.
fn requires_auth(opcode: Opcode) -> bool {
    !matches!(
        opcode,
        Opcode::Hello | Opcode::Auth | Opcode::Ping | Opcode::Health
    )
}

/// Verify a presented token against the configured token hash. Returns false if
/// auth is misconfigured (no hash) rather than panicking; config validation
/// guarantees a hash is present whenever auth is enabled.
fn verify_session_token(ctx: &ServerContext, token: &str) -> bool {
    match ctx.config.auth.token_hash.as_deref() {
        Some(hash) => crate::auth::verify_token(hash, token).unwrap_or(false),
        None => false,
    }
}

fn handle(ctx: &ServerContext, session: &mut Session, frame: &Frame) -> Result<Frame> {
    // Fail closed: when authentication is enabled, gated operations are refused
    // until the session is authenticated.
    if ctx.config.auth.enabled && !session.authenticated && requires_auth(frame.opcode) {
        Metrics::incr(&ctx.metrics.auth_failures_total);
        return Err(Error::Unauthenticated(
            "authentication required before this operation".into(),
        ));
    }

    match frame.opcode {
        Opcode::Hello => {
            let req: HelloRequest = frame.decode_json()?;
            let negotiated = req.protocol_version.min(PROTOCOL_VERSION);
            session.negotiated_version = negotiated;
            let auth_required = ctx.config.auth.enabled;
            if !auth_required {
                session.authenticated = true;
            } else if let Some(token) = req.auth_token.as_deref() {
                // Fast-path authentication carried in the handshake.
                if verify_session_token(ctx, token) {
                    session.authenticated = true;
                } else {
                    Metrics::incr(&ctx.metrics.auth_failures_total);
                }
            }
            let ack = HelloAck {
                protocol_version: negotiated,
                capabilities: ServerCapabilities::current(PROTOCOL_VERSION),
                auth_required,
                authenticated: session.authenticated,
            };
            Frame::json(Opcode::HelloAck, frame.request_id, 0, &ack)
        }
        Opcode::Auth => {
            let req: AuthRequest = frame.decode_json()?;
            if !ctx.config.auth.enabled || verify_session_token(ctx, &req.token) {
                session.authenticated = true;
                Frame::json(
                    Opcode::AuthResult,
                    frame.request_id,
                    0,
                    &AuthResult {
                        authenticated: true,
                    },
                )
            } else {
                Metrics::incr(&ctx.metrics.auth_failures_total);
                Err(Error::InvalidCredentials)
            }
        }
        Opcode::Ping => Ok(Frame::new(
            Opcode::Pong,
            frame.request_id,
            frame.txn_id,
            frame.payload.clone(),
        )),
        Opcode::Health => {
            let stats = ctx.engine.stats();
            let report = HealthReport {
                status: HealthStatus::Healthy,
                ready: true,
                version: env!("CARGO_PKG_VERSION").to_string(),
                collections: stats.collections,
                mvcc: Some(MvccHealth {
                    active_transactions: stats.active_transactions,
                    timed_out_transactions: stats.timed_out_transactions,
                    oldest_active_read_ts: stats.oldest_active_read_ts,
                    oldest_transaction_age_secs: stats.oldest_transaction_age_secs,
                    retained_versions: stats.versions,
                    transaction_timeouts_total: stats.transaction_timeouts_total,
                    transaction_timeout_secs: ctx.config.mvcc.transaction_timeout_secs,
                    gc_enabled: ctx.config.mvcc.gc_enabled,
                }),
                cluster: cluster_health(ctx),
            };
            Frame::json(Opcode::HealthResult, frame.request_id, 0, &report)
        }
        Opcode::SchemaCreate => {
            let schema: CollectionSchema = frame.decode_json()?;
            ctx.engine.create_schema(schema)?;
            ok_ok(frame)
        }
        Opcode::SchemaDrop => {
            let payload: NamePayload = frame.decode_json()?;
            ctx.engine.drop_schema(&payload.name)?;
            ok_ok(frame)
        }
        Opcode::SchemaGet => {
            let payload: NamePayload = frame.decode_json()?;
            let schema = ctx
                .engine
                .get_schema(&payload.name)
                .ok_or_else(|| Error::NotFound(format!("collection {}", payload.name)))?;
            Frame::json(Opcode::Ok, frame.request_id, 0, &schema)
        }
        Opcode::SchemaList => {
            let schemas = ctx.engine.list_schemas();
            Frame::json(Opcode::Ok, frame.request_id, 0, &schemas)
        }
        Opcode::Query => {
            Metrics::incr(&ctx.metrics.queries_total);
            let req: ReadRequest = frame.decode_json()?;
            handle_query(ctx, session, frame, req)
        }
        Opcode::Mutate => {
            Metrics::incr(&ctx.metrics.mutations_total);
            let mutation: Mutation = frame.decode_json()?;
            handle_mutation(ctx, session, frame, mutation)
        }
        Opcode::CursorFetch => {
            let req: CursorFetchRequest = frame.decode_json()?;
            // A cursor opened inside a transaction is paged through that same
            // transaction so staged writes stay visible across fetches.
            let page = if frame.txn_id != 0 {
                let txn = session
                    .transactions
                    .get(&frame.txn_id)
                    .ok_or_else(|| Error::NotFound(format!("transaction {}", frame.txn_id)))?;
                ctx.cursors
                    .fetch_with(req.cursor_id, req.limit, &ctx.engine, Some(txn))?
            } else {
                ctx.cursors.fetch(req.cursor_id, req.limit, &ctx.engine)?
            };
            if !page.more {
                session.cursor_ids.remove(&req.cursor_id);
            }
            Metrics::gauge_set(&ctx.metrics.active_cursors, ctx.cursors.len() as u64);
            let result = QueryResultPage {
                rows: page.rows,
                cursor_id: if page.more { Some(req.cursor_id) } else { None },
            };
            Frame::json(Opcode::QueryResult, frame.request_id, frame.txn_id, &result)
        }
        Opcode::CursorClose => {
            let req: CursorCloseRequest = frame.decode_json()?;
            ctx.cursors.close(req.cursor_id);
            session.cursor_ids.remove(&req.cursor_id);
            Metrics::gauge_set(&ctx.metrics.active_cursors, ctx.cursors.len() as u64);
            ok_ok(frame)
        }
        Opcode::Explain => {
            // The Explain payload is the raw Query IR. An optional sibling
            // `"analyze": true` key requests EXPLAIN ANALYZE (execute and report
            // metrics). Carrying the flag inside the IR object keeps older
            // connectors — which send a bare FindQuery — compatible: they simply
            // omit the key and get a plan without execution metrics.
            let value: serde_json::Value = frame.decode_json()?;
            let analyze = value
                .get("analyze")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let query: FindQuery = serde_json::from_value(value)
                .map_err(|e| Error::InvalidRequest(format!("invalid explain query: {e}")))?;
            let plan = match (frame.txn_id != 0, analyze) {
                (true, _) => {
                    let txn = session
                        .transactions
                        .get(&frame.txn_id)
                        .ok_or_else(|| Error::NotFound(format!("transaction {}", frame.txn_id)))?;
                    if analyze {
                        ctx.engine.txn_explain_analyze(txn, &query)?
                    } else {
                        ctx.engine.txn_explain(txn, &query)?
                    }
                }
                (false, true) => ctx.engine.explain_analyze(&query)?,
                (false, false) => ctx.engine.explain(&query)?,
            };
            Frame::json(Opcode::Ok, frame.request_id, 0, &plan)
        }
        Opcode::MigrationEstimate => {
            let target: CollectionSchema = frame.decode_json()?;
            let estimate = ctx.engine.migration_estimate(&target)?;
            Frame::json(Opcode::Ok, frame.request_id, 0, &estimate)
        }
        Opcode::TxnBegin => {
            let txn = ctx
                .engine
                .begin_with_connection(Some(session.connection_id));
            let id = txn.id().get();
            session.transactions.insert(id, txn);
            Metrics::gauge_set(
                &ctx.metrics.active_transactions,
                session.transactions.len() as u64,
            );
            Frame::json(
                Opcode::Ok,
                frame.request_id,
                id,
                &TxnBeginResult { txn_id: id },
            )
        }
        Opcode::TxnCommit => {
            let txn = session
                .transactions
                .remove(&frame.txn_id)
                .ok_or_else(|| Error::NotFound(format!("transaction {}", frame.txn_id)))?;
            ctx.engine.commit(txn)?;
            Metrics::gauge_set(
                &ctx.metrics.active_transactions,
                session.transactions.len() as u64,
            );
            ok_ok(frame)
        }
        Opcode::TxnRollback => {
            let txn = session
                .transactions
                .remove(&frame.txn_id)
                .ok_or_else(|| Error::NotFound(format!("transaction {}", frame.txn_id)))?;
            ctx.engine.rollback(txn);
            Metrics::gauge_set(
                &ctx.metrics.active_transactions,
                session.transactions.len() as u64,
            );
            ok_ok(frame)
        }
        // Response opcodes are never received as requests.
        Opcode::HelloAck
        | Opcode::Pong
        | Opcode::HealthResult
        | Opcode::AuthResult
        | Opcode::Ok
        | Opcode::QueryResult
        | Opcode::Error => Err(Error::Protocol(format!(
            "opcode 0x{:02x} is a response, not a request",
            frame.opcode.as_u8()
        ))),
    }
}

fn handle_query(
    ctx: &ServerContext,
    session: &mut Session,
    frame: &Frame,
    req: ReadRequest,
) -> Result<Frame> {
    // When a transaction id is present, every read executes against that
    // transaction's view (committed state overlaid with its staged writes and
    // deletes). Non-transactional reads (txn_id == 0) keep their prior path.
    match req {
        ReadRequest::Find(query) => {
            let page_size = ctx.config.page_size;
            // Plan and materialize the first page, against the transaction view
            // when one applies. Results are owned, so the transaction borrow is
            // released before the cursor is registered below.
            let (rows, remaining) = if frame.txn_id != 0 {
                let txn = session
                    .transactions
                    .get(&frame.txn_id)
                    .ok_or_else(|| Error::NotFound(format!("transaction {}", frame.txn_id)))?;
                let planned = ctx.engine.txn_plan_find(txn, &query)?;
                let first_end = planned.ordered.len().min(page_size);
                let rows =
                    ctx.engine
                        .txn_materialize(txn, &query, &planned.ordered[..first_end])?;
                (rows, planned.ordered[first_end..].to_vec())
            } else {
                let planned = ctx.engine.plan_find(&query)?;
                let first_end = planned.ordered.len().min(page_size);
                let rows = ctx
                    .engine
                    .materialize(&query, &planned.ordered[..first_end])?;
                (rows, planned.ordered[first_end..].to_vec())
            };
            let cursor_id = if !remaining.is_empty() {
                let id = ctx.cursors.open(query.clone(), remaining);
                session.cursor_ids.insert(id);
                Metrics::gauge_set(&ctx.metrics.active_cursors, ctx.cursors.len() as u64);
                Some(id)
            } else {
                None
            };
            let result = QueryResultPage { rows, cursor_id };
            Frame::json(Opcode::QueryResult, frame.request_id, frame.txn_id, &result)
        }
        ReadRequest::Count(query) => {
            let count = if frame.txn_id != 0 {
                let txn = session
                    .transactions
                    .get(&frame.txn_id)
                    .ok_or_else(|| Error::NotFound(format!("transaction {}", frame.txn_id)))?;
                ctx.engine.txn_count(txn, &query)?
            } else {
                ctx.engine.count(&query)?
            };
            Frame::json(
                Opcode::Ok,
                frame.request_id,
                frame.txn_id,
                &serde_json::json!({ "count": count }),
            )
        }
        ReadRequest::Exists(query) => {
            let exists = if frame.txn_id != 0 {
                let txn = session
                    .transactions
                    .get(&frame.txn_id)
                    .ok_or_else(|| Error::NotFound(format!("transaction {}", frame.txn_id)))?;
                ctx.engine.txn_exists(txn, &query)?
            } else {
                ctx.engine.exists(&query)?
            };
            Frame::json(
                Opcode::Ok,
                frame.request_id,
                frame.txn_id,
                &serde_json::json!({ "exists": exists }),
            )
        }
    }
}

fn handle_mutation(
    ctx: &ServerContext,
    session: &mut Session,
    frame: &Frame,
    mutation: Mutation,
) -> Result<Frame> {
    let result = if frame.txn_id != 0 {
        let txn = session
            .transactions
            .get_mut(&frame.txn_id)
            .ok_or_else(|| Error::NotFound(format!("transaction {}", frame.txn_id)))?;
        ctx.engine.stage(txn, mutation)?
    } else {
        ctx.engine.apply_mutation(mutation)?
    };
    Frame::json(Opcode::Ok, frame.request_id, frame.txn_id, &result)
}
