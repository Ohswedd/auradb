//! Request dispatch: maps a decoded request frame to a response frame.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use auradb::core::CollectionSchema;
use auradb::query::{FindQuery, Mutation, QueryResultPage, ReadRequest};
use auradb::{Engine, Transaction};
use auradb_core::{Error, Result, ServerCapabilities};
use auradb_observability::Metrics;
use auradb_protocol::{
    CursorCloseRequest, CursorFetchRequest, ErrorPayload, Frame, HealthReport, HealthStatus,
    HelloAck, HelloRequest, Opcode, PROTOCOL_VERSION,
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
}

/// Per-connection mutable state.
#[derive(Default)]
pub struct Session {
    /// Negotiated protocol version (0 until HELLO).
    pub negotiated_version: u8,
    /// Open transactions keyed by transaction id.
    pub transactions: HashMap<u64, Transaction>,
    /// Cursor ids owned by this connection (for cleanup on disconnect).
    pub cursor_ids: HashSet<u64>,
}

impl Session {
    /// Clean up resources owned by this session (called on disconnect).
    pub fn cleanup(&mut self, ctx: &ServerContext) {
        for id in self.cursor_ids.drain() {
            ctx.cursors.close(id);
        }
        self.transactions.clear();
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
            tracing::warn!(request_id = frame.request_id.0, code = %err.code(), error = %err, "request failed");
            ErrorPayload::from_error(&err).to_frame(frame.request_id, frame.txn_id)
        }
    }
}

fn handle(ctx: &ServerContext, session: &mut Session, frame: &Frame) -> Result<Frame> {
    match frame.opcode {
        Opcode::Hello => {
            let req: HelloRequest = frame.decode_json()?;
            let negotiated = req.protocol_version.min(PROTOCOL_VERSION);
            session.negotiated_version = negotiated;
            let ack = HelloAck {
                protocol_version: negotiated,
                capabilities: ServerCapabilities::current(PROTOCOL_VERSION),
            };
            Frame::json(Opcode::HelloAck, frame.request_id, 0, &ack)
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
            let page = ctx.cursors.fetch(req.cursor_id, req.limit, &ctx.engine)?;
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
            let query: FindQuery = frame.decode_json()?;
            let plan = ctx.engine.explain(&query)?;
            Frame::json(Opcode::Ok, frame.request_id, 0, &plan)
        }
        Opcode::MigrationEstimate => {
            let target: CollectionSchema = frame.decode_json()?;
            let estimate = ctx.engine.migration_estimate(&target)?;
            Frame::json(Opcode::Ok, frame.request_id, 0, &estimate)
        }
        Opcode::TxnBegin => {
            let txn = ctx.engine.begin();
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
    match req {
        ReadRequest::Find(query) => {
            let planned = ctx.engine.plan_find(&query)?;
            let page_size = ctx.config.page_size;
            let first_end = planned.ordered.len().min(page_size);
            let rows = ctx
                .engine
                .materialize(&query, &planned.ordered[..first_end])?;
            let cursor_id = if planned.ordered.len() > first_end {
                let id = ctx
                    .cursors
                    .open(query.clone(), planned.ordered[first_end..].to_vec());
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
            let count = ctx.engine.count(&query)?;
            Frame::json(
                Opcode::Ok,
                frame.request_id,
                0,
                &serde_json::json!({ "count": count }),
            )
        }
        ReadRequest::Exists(query) => {
            let exists = ctx.engine.exists(&query)?;
            Frame::json(
                Opcode::Ok,
                frame.request_id,
                0,
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
