//! Typed errors for the replication layer.

/// Errors raised while encoding, decoding, or applying replicated commands.
#[derive(Debug, thiserror::Error)]
pub enum ReplicationError {
    /// A replicated command failed to encode or decode.
    #[error("replicated command codec error: {0}")]
    Codec(String),

    /// A replicated command envelope declared an unknown future version.
    #[error("replicated command envelope version {found} is newer than supported {supported}")]
    UnsupportedVersion {
        /// The version found.
        found: u16,
        /// The newest version understood.
        supported: u16,
    },

    /// The Raft core reported an error.
    #[error(transparent)]
    Raft(#[from] auradb_raft::RaftError),

    /// The engine reported an error while applying a committed command.
    #[error("engine apply error: {0}")]
    Apply(#[from] auradb_core::Error),

    /// A snapshot was rejected because its format version is unsupported.
    #[error("snapshot format version {found} is newer than supported {supported}")]
    UnsupportedSnapshot {
        /// The version found.
        found: u32,
        /// The newest version understood.
        supported: u32,
    },

    /// A snapshot manifest or payload was malformed.
    #[error("snapshot is malformed: {0}")]
    SnapshotMalformed(String),
}

/// Result alias for replication operations.
pub type Result<T> = std::result::Result<T, ReplicationError>;
