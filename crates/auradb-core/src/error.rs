//! Error and result types shared across AuraDB crates.
//!
//! Every fallible boundary in the engine returns [`Error`]. Each variant maps to
//! a stable [`ErrorCode`] so the protocol layer can serialize structured error
//! frames with codes that clients can match on.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable, machine-readable error codes carried in protocol error frames.
///
/// These values are part of the wire contract: their numeric representation
/// must remain stable across releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// A request was structurally invalid (bad arguments, malformed IR).
    InvalidRequest,
    /// A protocol frame failed decoding or validation.
    Protocol,
    /// Persistent storage failed or detected corruption.
    Storage,
    /// On-disk data failed a checksum or structural integrity check.
    Corruption,
    /// A transaction could not be committed because of a conflict.
    Conflict,
    /// A transaction exceeded its idle timeout and was aborted.
    TransactionTimeout,
    /// A schema constraint was violated.
    SchemaViolation,
    /// A uniqueness constraint was violated.
    UniqueViolation,
    /// A referenced object (collection, record, schema, cursor) was not found.
    NotFound,
    /// The requested feature is recognized but not supported in this release.
    Unsupported,
    /// A request requires authentication but the session is not authenticated.
    Unauthenticated,
    /// Authentication failed because the presented credentials were invalid.
    InvalidCredentials,
    /// A configuration value was invalid.
    Config,
    /// An I/O operation failed.
    Io,
    /// The request exceeded a configured limit (payload size, query memory).
    LimitExceeded,
    /// An internal invariant was violated. Indicates a bug.
    Internal,
}

impl ErrorCode {
    /// The stable string identifier for this code.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::Protocol => "protocol",
            ErrorCode::Storage => "storage",
            ErrorCode::Corruption => "corruption",
            ErrorCode::Conflict => "conflict",
            ErrorCode::TransactionTimeout => "transaction_timeout",
            ErrorCode::SchemaViolation => "schema_violation",
            ErrorCode::UniqueViolation => "unique_violation",
            ErrorCode::NotFound => "not_found",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::Unauthenticated => "unauthenticated",
            ErrorCode::InvalidCredentials => "invalid_credentials",
            ErrorCode::Config => "config",
            ErrorCode::Io => "io",
            ErrorCode::LimitExceeded => "limit_exceeded",
            ErrorCode::Internal => "internal",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error type returned throughout AuraDB.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A request was structurally invalid.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// A protocol frame failed decoding or validation.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// Persistent storage failed.
    #[error("storage error: {0}")]
    Storage(String),

    /// On-disk data failed an integrity check.
    #[error("corruption detected: {0}")]
    Corruption(String),

    /// A transaction conflict prevented commit.
    #[error("transaction conflict: {0}")]
    Conflict(String),

    /// A transaction exceeded its idle timeout and was aborted; its snapshot has
    /// been released and no further operations on it will be accepted.
    #[error("transaction timed out: {0}")]
    TransactionTimeout(String),

    /// A schema constraint was violated.
    #[error("schema violation: {0}")]
    SchemaViolation(String),

    /// A uniqueness constraint was violated.
    #[error("unique constraint violation: {0}")]
    UniqueViolation(String),

    /// A referenced object was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// A recognized but unimplemented feature was requested.
    #[error("unsupported: {feature} (this single-node release does not implement this)")]
    Unsupported {
        /// The capability that was requested but is not supported.
        feature: String,
    },

    /// Authentication is required but was not provided or not completed.
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),

    /// The presented authentication credentials were invalid.
    #[error("invalid credentials")]
    InvalidCredentials,

    /// A configuration value was invalid.
    #[error("configuration error: {0}")]
    Config(String),

    /// An I/O operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A request exceeded a configured limit.
    #[error("limit exceeded: {0}")]
    LimitExceeded(String),

    /// An internal invariant was violated.
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Construct an [`Error::Unsupported`] for a named capability.
    pub fn unsupported(feature: impl Into<String>) -> Self {
        Error::Unsupported {
            feature: feature.into(),
        }
    }

    /// The stable [`ErrorCode`] associated with this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            Error::InvalidRequest(_) => ErrorCode::InvalidRequest,
            Error::Protocol(_) => ErrorCode::Protocol,
            Error::Storage(_) => ErrorCode::Storage,
            Error::Corruption(_) => ErrorCode::Corruption,
            Error::Conflict(_) => ErrorCode::Conflict,
            Error::TransactionTimeout(_) => ErrorCode::TransactionTimeout,
            Error::SchemaViolation(_) => ErrorCode::SchemaViolation,
            Error::UniqueViolation(_) => ErrorCode::UniqueViolation,
            Error::NotFound(_) => ErrorCode::NotFound,
            Error::Unsupported { .. } => ErrorCode::Unsupported,
            Error::Unauthenticated(_) => ErrorCode::Unauthenticated,
            Error::InvalidCredentials => ErrorCode::InvalidCredentials,
            Error::Config(_) => ErrorCode::Config,
            Error::Io(_) => ErrorCode::Io,
            Error::LimitExceeded(_) => ErrorCode::LimitExceeded,
            Error::Internal(_) => ErrorCode::Internal,
        }
    }
}

/// The standard result alias used across AuraDB.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable_strings() {
        assert_eq!(ErrorCode::Conflict.as_str(), "conflict");
        assert_eq!(ErrorCode::UniqueViolation.to_string(), "unique_violation");
    }

    #[test]
    fn errors_map_to_codes() {
        assert_eq!(Error::Conflict("x".into()).code(), ErrorCode::Conflict);
        assert_eq!(
            Error::unsupported("clustering").code(),
            ErrorCode::Unsupported
        );
    }

    #[test]
    fn error_code_roundtrips_through_json() {
        for code in [
            ErrorCode::InvalidRequest,
            ErrorCode::Storage,
            ErrorCode::Conflict,
            ErrorCode::Unsupported,
        ] {
            let json = serde_json::to_string(&code).unwrap();
            let back: ErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(code, back);
        }
    }

    #[test]
    fn unsupported_message_is_honest() {
        let msg = Error::unsupported("raft consensus").to_string();
        assert!(msg.contains("raft consensus"));
        assert!(msg.contains("does not implement"));
    }
}
