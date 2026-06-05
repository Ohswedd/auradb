//! Typed errors for the Raft log and consensus core.

use std::path::PathBuf;

/// Errors raised by the Raft log and node.
#[derive(Debug, thiserror::Error)]
pub enum RaftError {
    /// An I/O operation against the durable log or state failed.
    #[error("raft io error at {path}: {source}")]
    Io {
        /// The file involved.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },

    /// The durable log or state failed an integrity check.
    #[error("raft log corruption: {0}")]
    Corruption(String),

    /// An append violated log invariants (a gap or a term regression).
    #[error("invalid raft log append: {0}")]
    InvalidAppend(String),

    /// A write was attempted on a node that is not the leader.
    #[error("not leader: this node cannot accept writes{}", leader_hint(.0))]
    NotLeader(Option<auradb_cluster::NodeId>),

    /// An entry payload failed to encode or decode.
    #[error("raft command codec error: {0}")]
    Codec(String),
}

fn leader_hint(leader: &Option<auradb_cluster::NodeId>) -> String {
    match leader {
        Some(id) => format!(" (current leader: {id})"),
        None => String::new(),
    }
}

/// Result alias for Raft operations.
pub type Result<T> = std::result::Result<T, RaftError>;
