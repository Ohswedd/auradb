//! # auradb-protocol
//!
//! The Aura Wire Protocol (AWP): a binary, checksummed, versioned frame format
//! with opaque (JSON) payloads. This crate is transport-agnostic and synchronous;
//! the server crate provides async read/write helpers on top of [`Frame::decode`]
//! and [`Frame::encode`].
//!
//! Payloads are intentionally opaque here so this crate stays below the query
//! engine in the dependency graph. Connection-level payloads (hello, error,
//! cursor, health) live in [`message`].
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod frame;
pub mod message;
pub mod opcode;

pub use frame::{
    Compression, Frame, RequestId, DEFAULT_MAX_PAYLOAD, FLAG_PAYLOAD_CHECKSUM, HEADER_LEN, MAGIC,
    PROTOCOL_VERSION,
};
pub use message::{
    AuthRequest, AuthResult, ClusterHealth, ClusterPeerHealth, ClusterSnapshotHealth,
    CursorCloseRequest, CursorFetchRequest, ErrorPayload, HealthReport, HealthStatus, HelloAck,
    HelloRequest, MvccHealth, NotLeaderDetails,
};
pub use opcode::Opcode;
