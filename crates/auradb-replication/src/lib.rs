//! # auradb-replication
//!
//! Maps AuraDB database mutations onto a replicated Raft log and applies
//! committed entries back to the engine.
//!
//! This crate is the bridge between `auradb-raft` (consensus over an opaque log)
//! and `auradb` (the engine that holds data). It provides:
//!
//! - the [`ReplicatedCommand`] model and its framed, versioned encoding into a
//!   Raft command;
//! - the [`apply_command`] path that applies committed commands to the engine
//!   idempotently;
//! - a durable single-node [`ClusterNode`] coordinator the server uses when
//!   cluster mode is enabled with no peers;
//! - the [`SnapshotManifest`] boundary for future state transfer.
//!
//! ## Honest scope
//!
//! Single-node cluster mode is real and durable: writes are ordered through the
//! Raft log and replayed on restart. Multi-node consensus and the replicated
//! apply path are exercised by deterministic in-process tests. Cross-process
//! multi-node *server* transport is **not** part of v0.4.0; the recommended
//! production deployment remains single-node.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod apply;
mod command;
mod error;
mod node;
mod snapshot;

pub use apply::apply_command;
pub use command::{ReplicatedCommand, SchemaCommand, ENVELOPE_VERSION};
pub use error::{ReplicationError, Result};
pub use node::{ClusterNode, CompactionReport, ReplicationMetrics};
pub use snapshot::{RestoreOptions, SnapshotManifest, SnapshotMeta, SNAPSHOT_FORMAT_VERSION};
