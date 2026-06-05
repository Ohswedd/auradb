//! Typed errors for cluster identity, metadata, and configuration.

use std::path::PathBuf;

/// Errors raised while loading, validating, or persisting cluster state.
#[derive(Debug, thiserror::Error)]
pub enum ClusterError {
    /// A cluster metadata or identity file could not be read or written.
    #[error("cluster io error at {path}: {source}")]
    Io {
        /// The file involved.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// A persisted cluster file failed to decode.
    #[error("cluster metadata at {path} is corrupt or malformed: {detail}")]
    Corrupt {
        /// The file involved.
        path: PathBuf,
        /// What was wrong.
        detail: String,
    },

    /// A persisted file declares a format version this build cannot read.
    ///
    /// AuraDB fails closed on unknown future formats rather than guessing.
    #[error(
        "cluster metadata at {path} declares format version {found}, but this build supports up \
         to {supported}; refusing to open data written by a newer AuraDB"
    )]
    UnsupportedFormat {
        /// The file involved.
        path: PathBuf,
        /// The version found on disk.
        found: u32,
        /// The newest version this build understands.
        supported: u32,
    },

    /// Two on-disk identity records disagree, or an identity is otherwise invalid.
    #[error("conflicting or invalid cluster identity: {0}")]
    IdentityConflict(String),

    /// A configuration value was invalid or internally inconsistent.
    #[error("invalid cluster configuration: {0}")]
    Config(String),
}

/// Result alias for cluster operations.
pub type Result<T> = std::result::Result<T, ClusterError>;
