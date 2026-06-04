//! Protocol-level message payloads that are independent of the query engine.
//!
//! Query- and mutation-specific payloads (the Query IR and its results) are
//! defined in `auradb-query` and serialized into frame payloads by the server,
//! keeping this crate below the query engine in the dependency graph.

use auradb_core::{Error, ErrorCode, ServerCapabilities};
use serde::{Deserialize, Serialize};

use crate::frame::{Frame, RequestId};
use crate::opcode::Opcode;

/// Client handshake payload (`HELLO`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloRequest {
    /// The client software version string.
    pub client_version: String,
    /// The highest protocol version the client supports.
    pub protocol_version: u8,
}

/// Server handshake acknowledgement payload (`HELLO_ACK`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloAck {
    /// The negotiated protocol version (min of client and server max).
    pub protocol_version: u8,
    /// The server's advertised capabilities.
    pub capabilities: ServerCapabilities,
}

/// Structured error payload carried in an [`Opcode::Error`] frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorPayload {
    /// The stable error code.
    pub code: ErrorCode,
    /// A human-readable message.
    pub message: String,
}

impl ErrorPayload {
    /// Build an error payload from an engine [`Error`].
    pub fn from_error(err: &Error) -> Self {
        ErrorPayload {
            code: err.code(),
            message: err.to_string(),
        }
    }

    /// Encode this payload as an [`Opcode::Error`] frame for `request_id`.
    pub fn to_frame(&self, request_id: RequestId, txn_id: u64) -> Frame {
        Frame::json(Opcode::Error, request_id, txn_id, self)
            .expect("error payload always serializes")
    }
}

/// Request to fetch the next page from a server-side cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorFetchRequest {
    /// The cursor identifier returned in a prior query result.
    pub cursor_id: u64,
    /// The maximum number of rows to return in this page.
    pub limit: usize,
}

/// Request to close a server-side cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorCloseRequest {
    /// The cursor identifier to close.
    pub cursor_id: u64,
}

/// Health status levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// Fully operational.
    Healthy,
    /// Operating in a degraded but serving state.
    Degraded,
}

/// Health / readiness report payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthReport {
    /// Overall status.
    pub status: HealthStatus,
    /// Whether the engine is ready to serve requests.
    pub ready: bool,
    /// Server version.
    pub version: String,
    /// Number of collections currently registered.
    pub collections: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::DEFAULT_MAX_PAYLOAD;

    #[test]
    fn error_payload_roundtrips_through_frame() {
        let err = Error::Conflict("write-write".into());
        let payload = ErrorPayload::from_error(&err);
        assert_eq!(payload.code, ErrorCode::Conflict);
        let frame = payload.to_frame(RequestId(5), 0);
        let bytes = frame.encode();
        let (decoded, _) = Frame::decode(&bytes, DEFAULT_MAX_PAYLOAD).unwrap().unwrap();
        assert_eq!(decoded.opcode, Opcode::Error);
        let back: ErrorPayload = decoded.decode_json().unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn cursor_messages_roundtrip() {
        let req = CursorFetchRequest {
            cursor_id: 7,
            limit: 100,
        };
        let frame = Frame::json(Opcode::CursorFetch, RequestId::ZERO, 0, &req).unwrap();
        let back: CursorFetchRequest = frame.decode_json().unwrap();
        assert_eq!(back, req);

        let close = CursorCloseRequest { cursor_id: 7 };
        let frame = Frame::json(Opcode::CursorClose, RequestId::ZERO, 0, &close).unwrap();
        let back: CursorCloseRequest = frame.decode_json().unwrap();
        assert_eq!(back, close);
    }

    #[test]
    fn hello_roundtrips() {
        let ack = HelloAck {
            protocol_version: 1,
            capabilities: ServerCapabilities::current(1),
        };
        let frame = Frame::json(Opcode::HelloAck, RequestId::ZERO, 0, &ack).unwrap();
        let back: HelloAck = frame.decode_json().unwrap();
        assert_eq!(back, ack);
    }
}
