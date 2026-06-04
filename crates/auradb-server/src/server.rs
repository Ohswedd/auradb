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
use tokio::net::{TcpListener, TcpStream};

use crate::config::Config;
use crate::cursor::CursorRegistry;
use crate::dispatch::{respond, ServerContext, Session};
use crate::wire::{read_frame, write_frame};

/// A configured AuraDB server.
pub struct Server {
    ctx: ServerContext,
}

impl Server {
    /// Build a server from a validated configuration, opening the engine.
    pub fn open(config: Config) -> Result<Server> {
        config.validate()?;
        let engine = Engine::open_with(
            &config.data_dir,
            EngineOptions {
                storage: StorageOptions {
                    sync_on_commit: config.sync_on_commit,
                },
            },
        )?;
        let cursors = Arc::new(CursorRegistry::new(Duration::from_secs(
            config.cursor_timeout_secs,
        )));
        let metrics = Arc::new(Metrics::new());
        let ctx = ServerContext {
            engine,
            metrics,
            cursors,
            config: Arc::new(config),
        };
        Ok(Server { ctx })
    }

    /// The shared server context.
    pub fn context(&self) -> &ServerContext {
        &self.ctx
    }

    /// A snapshot of current metrics.
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        self.ctx.metrics.snapshot()
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
        tokio::select! {
            result = self.accept_loop(&listener) => {
                reaper.abort();
                result
            }
            _ = shutdown => {
                tracing::info!("shutdown signal received; stopping accept loop");
                reaper.abort();
                Ok(())
            }
        }
    }

    async fn accept_loop(&self, listener: &TcpListener) -> Result<()> {
        loop {
            let (socket, peer) = listener.accept().await?;
            let ctx = self.ctx.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(ctx, socket).await {
                    tracing::debug!(%peer, error = %e, "connection ended with error");
                }
            });
        }
    }
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

async fn handle_connection(ctx: ServerContext, socket: TcpStream) -> Result<()> {
    socket.set_nodelay(true).ok();
    let (mut reader, mut writer) = socket.into_split();
    Metrics::gauge_inc(&ctx.metrics.active_connections);
    let mut session = Session::default();
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
