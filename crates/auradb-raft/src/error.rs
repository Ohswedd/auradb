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

    /// A read or truncate targeted an index at or below the compacted prefix:
    /// those entries have been discarded after a snapshot covered them. The log
    /// fails closed with this structured error rather than returning a wrong or
    /// empty result.
    #[error(
        "raft log index {requested} is at or below the compacted prefix \
         (last included index {last_included}); those entries are no longer present"
    )]
    Compacted {
        /// The index that was requested.
        requested: u64,
        /// The last index included in the compacted prefix (covered by a snapshot).
        last_included: u64,
    },

    /// A compaction request was refused because it would discard entries that are
    /// not yet safely applied, are beyond the committed index, or are beyond the
    /// end of the log. Compaction never moves ahead of durability.
    #[error("raft log compaction refused: {0}")]
    CompactionRefused(String),

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
