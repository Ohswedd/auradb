//! The TCP server runtime: accept loop, per-connection tasks, a cursor reaper,
//! and graceful shutdown.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use auradb::storage::StorageOptions;
use auradb::{Engine, EngineOptions};
use auradb_core::Result;
use auradb_observability::{Metrics, MetricsSnapshot};
use auradb_protocol::{ErrorPayload, RequestId};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use crate::config::Config;
use crate::cursor::CursorRegistry;
use crate::dispatch::{respond, ServerContext, Session};
use crate::wire::{read_frame, write_frame};

/// A configured AuraDB server.
pub struct Server {
    ctx: ServerContext,
    tls: Option<TlsAcceptor>,
}

impl Server {
    /// Build a server from a validated configuration, opening the engine.
    pub fn open(config: Config) -> Result<Server> {
        config.validate()?;
        // Build the TLS acceptor before opening anything else so an invalid
        // certificate aborts startup (fail closed) rather than serving plaintext.
        let tls = if config.tls.enabled {
            Some(crate::tls::build_acceptor(&config.tls)?)
        } else {
            None
        };
        let engine = Engine::open_with(
            &config.data_dir,
            EngineOptions {
                storage: StorageOptions {
                    sync_on_commit: config.sync_on_commit,
                },
                gc_min_retained_versions: config.mvcc.min_retained_versions,
                transaction_timeout_secs: config.mvcc.transaction_timeout_secs,
                clock: auradb::WallClock::System,
            },
        )?;
        let cursors = Arc::new(CursorRegistry::new(Duration::from_secs(
            config.cursor_timeout_secs,
        )));
        let metrics = Arc::new(Metrics::new());

        // When cluster mode is enabled, bootstrap a durable single-node Raft
        // deployment and route the engine's writes through it. (Multi-node
        // deployment is rejected during config validation above, so reaching here
        // with cluster mode on always means a single-node cluster.)
        let cluster = if config.cluster.enabled {
            Some(build_cluster_node(&engine, &config)?)
        } else {
            None
        };

        let ctx = ServerContext {
            engine,
            metrics,
            cursors,
            config: Arc::new(config),
            connection_counter: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cluster,
        };
        Ok(Server { ctx, tls })
    }

    /// Whether this server terminates TLS.
    pub fn tls_enabled(&self) -> bool {
        self.tls.is_some()
    }

    /// The shared server context.
    pub fn context(&self) -> &ServerContext {
        &self.ctx
    }

    /// A snapshot of current metrics (refreshing cluster gauges first).
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        self.ctx.refresh_cluster_metrics();
        self.ctx.metrics.snapshot()
    }

    /// The cluster status snapshot, when cluster mode is enabled.
    pub fn cluster_status(&self) -> Option<auradb_cluster::ClusterStatus> {
        self.ctx.cluster.as_ref().map(|c| c.status())
    }

    /// Bind to the configured address and serve until `shutdown` resolves.
    pub async fn run<F: Future<Output = ()>>(&self, shutdown: F) -> Result<()> {
        let listener = TcpListener::bind(self.ctx.config.socket_addr()).await?;
        tracing::info!(addr = %self.ctx.config.socket_addr(), "AuraDB server listening");
        self.run_on(listener, shutdown).await
    }

    /// Serve on a pre-bound listener until `shutdown` resolves. Useful for tests
    /// that bind an ephemeral port.
    pub async fn run_on<F: Future<Output = ()>>(
        &self,
        listener: TcpListener,
        shutdown: F,
    ) -> Result<()> {
        let reaper = spawn_reaper(self.ctx.clone());
        let gc = spawn_gc(self.ctx.clone());
        let txn_reaper = spawn_txn_reaper(self.ctx.clone());
        let result = tokio::select! {
            result = self.accept_loop(&listener) => {
                reaper.abort();
                if let Some(gc) = &gc { gc.abort(); }
                if let Some(t) = &txn_reaper { t.abort(); }
                result
            }
            _ = shutdown => {
                tracing::info!("shutdown signal received; stopping accept loop");
                reaper.abort();
                if let Some(gc) = &gc { gc.abort(); }
                if let Some(t) = &txn_reaper { t.abort(); }
                Ok(())
            }
        };
        // Persist a durable index checkpoint so the next open loads snapshots
        // rather than rebuilding from storage.
        if let Err(e) = self.ctx.engine.checkpoint() {
            tracing::warn!(error = %e, "index checkpoint on shutdown failed");
        }
        result
    }

    async fn accept_loop(&self, listener: &TcpListener) -> Result<()> {
        loop {
            let (socket, peer) = listener.accept().await?;
            socket.set_nodelay(true).ok();
            let ctx = self.ctx.clone();
            let tls = self.tls.clone();
            tokio::spawn(async move {
                match tls {
                    Some(acceptor) => match acceptor.accept(socket).await {
                        Ok(stream) => {
                            if let Err(e) = handle_connection(ctx, stream).await {
                                tracing::debug!(%peer, error = %e, "connection ended with error");
                            }
                        }
                        Err(e) => {
                            tracing::debug!(%peer, error = %e, "TLS handshake failed");
                        }
                    },
                    None => {
                        if let Err(e) = handle_connection(ctx, socket).await {
                            tracing::debug!(%peer, error = %e, "connection ended with error");
                        }
                    }
                }
            });
        }
    }
}

/// Bootstrap (or recover) the single-node cluster coordinator and route the
/// engine's writes through it. Identity is created on first run and reused
/// afterwards; a config-pinned id that conflicts with persisted identity fails
/// closed.
fn build_cluster_node(
    engine: &Engine,
    config: &Config,
) -> Result<Arc<auradb_replication::ClusterNode>> {
    use auradb_core::Error;

    let version = env!("CARGO_PKG_VERSION");
    let store = auradb_cluster::ClusterStore::new(&config.data_dir);
    let pinned_node = parse_opt::<auradb_cluster::NodeId>(&config.cluster.node_id, "node_id")?;
    let pinned_cluster =
        parse_opt::<auradb_cluster::ClusterId>(&config.cluster.cluster_id, "cluster_id")?;
    let identity = store
        .init(pinned_node, pinned_cluster, version)
        .map_err(|e| Error::Config(e.to_string()))?;
    let node = auradb_replication::ClusterNode::bootstrap(
        engine.clone(),
        identity,
        config.cluster.clone(),
        store.dir(),
    )
    .map_err(|e| Error::Internal(format!("cluster bootstrap failed: {e}")))?;
    engine.attach_replicated_log(node.write_log());
    tracing::info!(
        node_id = %node.identity().node_id(),
        cluster_id = %node.identity().cluster_id(),
        "single-node cluster mode enabled; writes are routed through the Raft log"
    );
    Ok(Arc::new(node))
}

fn parse_opt<T: std::str::FromStr>(value: &str, field: &str) -> Result<Option<T>>
where
    T::Err: std::fmt::Display,
{
    if value.trim().is_empty() {
        return Ok(None);
    }
    value
        .trim()
        .parse::<T>()
        .map(Some)
        .map_err(|e| auradb_core::Error::Config(format!("invalid cluster.{field}: {e}")))
}

fn spawn_reaper(ctx: ServerContext) -> tokio::task::JoinHandle<()> {
    let interval_secs = (ctx.config.cursor_timeout_secs / 2).max(1);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            tick.tick().await;
            let reaped = ctx.cursors.reap();
            if reaped > 0 {
                tracing::debug!(reaped, "reaped idle cursors");
            }
            Metrics::gauge_set(&ctx.metrics.active_cursors, ctx.cursors.len() as u64);
        }
    })
}

/// Spawn the background version garbage-collector when enabled in `[mvcc]`. It
/// reclaims MVCC versions no active transaction can observe on a fixed interval.
fn spawn_gc(ctx: ServerContext) -> Option<tokio::task::JoinHandle<()>> {
    if !ctx.config.mvcc.gc_enabled {
        return None;
    }
    let interval_secs = ctx.config.mvcc.gc_interval_secs.max(1);
    Some(tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(interval_secs));
        // Skip the immediate first tick so startup isn't followed by a GC pass.
        tick.tick().await;
        loop {
            tick.tick().await;
            match ctx.engine.gc() {
                Ok(report) => {
                    Metrics::incr(&ctx.metrics.mvcc_gc_runs_total);
                    Metrics::add(
                        &ctx.metrics.mvcc_gc_reclaimed_versions_total,
                        report.versions_reclaimed as u64,
                    );
                    Metrics::add(
                        &ctx.metrics.mvcc_gc_reclaimed_bytes_total,
                        report.bytes_reclaimed,
                    );
                    if report.versions_reclaimed > 0 || report.records_removed > 0 {
                        tracing::debug!(
                            versions = report.versions_reclaimed,
                            records = report.records_removed,
                            bytes = report.bytes_reclaimed,
                            "background GC reclaimed old versions"
                        );
                    }
                    refresh_mvcc_metrics(&ctx);
                }
                Err(e) => tracing::warn!(error = %e, "background GC failed"),
            }
        }
    }))
}

/// Spawn the abandoned-transaction reaper when transaction timeouts are enabled.
/// On each pass it reaps transactions idle past `transaction_timeout_secs` —
/// releasing their pinned snapshots so GC can progress — and refreshes the MVCC
/// pressure gauges.
fn spawn_txn_reaper(ctx: ServerContext) -> Option<tokio::task::JoinHandle<()>> {
    if ctx.config.mvcc.transaction_timeout_secs == 0 {
        return None;
    }
    let interval_secs = ctx.config.mvcc.abandoned_transaction_reaper_secs.max(1);
    Some(tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            tick.tick().await;
            let reaped = ctx.engine.reap_transactions();
            if reaped > 0 {
                tracing::warn!(
                    reaped,
                    timeout_secs = ctx.config.mvcc.transaction_timeout_secs,
                    "reaped abandoned transactions past their idle timeout"
                );
            }
            refresh_mvcc_metrics(&ctx);
        }
    }))
}

/// Refresh the MVCC pressure gauges and the cumulative timeout counter from the
/// engine's current statistics.
fn refresh_mvcc_metrics(ctx: &ServerContext) {
    let stats = ctx.engine.stats();
    Metrics::gauge_set(
        &ctx.metrics.mvcc_active_transactions,
        stats.active_transactions as u64,
    );
    Metrics::gauge_set(
        &ctx.metrics.mvcc_oldest_snapshot_age_seconds,
        stats.oldest_transaction_age_secs.unwrap_or(0),
    );
    Metrics::gauge_set(&ctx.metrics.mvcc_retained_versions, stats.versions as u64);
    Metrics::gauge_set(
        &ctx.metrics.mvcc_transaction_timeouts_total,
        stats.transaction_timeouts_total,
    );
}

async fn handle_connection<S>(ctx: ServerContext, socket: S) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(socket);
    Metrics::gauge_inc(&ctx.metrics.active_connections);
    let mut session = Session {
        connection_id: ctx
            .connection_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ..Session::default()
    };
    let max_payload = ctx.config.max_payload_bytes;

    let result = loop {
        match read_frame(&mut reader, max_payload).await {
            Ok(Some(frame)) => {
                Metrics::add(&ctx.metrics.bytes_read, frame.encoded_len() as u64);
                let response = respond(&ctx, &mut session, frame);
                let written = write_frame(&mut writer, &response).await?;
                Metrics::add(&ctx.metrics.bytes_written, written as u64);
            }
            Ok(None) => break Ok(()),
            Err(err) => {
                // Send a best-effort error frame, then close (framing is no
                // longer trustworthy).
                let frame = ErrorPayload::from_error(&err).to_frame(RequestId::ZERO, 0);
                let _ = write_frame(&mut writer, &frame).await;
                break Err(err);
            }
        }
    };

    session.cleanup(&ctx);
    Metrics::gauge_dec(&ctx.metrics.active_connections);
    result
}
