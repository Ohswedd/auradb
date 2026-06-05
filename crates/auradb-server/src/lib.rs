//! # auradb-server
//!
//! The AuraDB network server. It binds a TCP listener, decodes Aura Wire
//! Protocol frames, dispatches them against the embeddable [`auradb::Engine`],
//! manages server-side cursors and per-connection transactions, records metrics,
//! and shuts down gracefully.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod auth;
mod cluster_runtime;
mod config;
mod cursor;
mod dispatch;
mod server;
mod tls;
mod wire;

pub use cluster_runtime::ClusterRuntime;
pub use config::{AuthConfig, AuthMode, Config, TlsConfig, TokenHashAlgorithm};
pub use cursor::{CursorPage, CursorRegistry};
pub use dispatch::{respond, ServerContext, Session};
pub use server::Server;
pub use wire::{read_frame, write_frame};
