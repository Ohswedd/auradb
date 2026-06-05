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
    /// An optional static authentication token presented at handshake time.
    ///
    /// Backward-compatible: clients that do not authenticate omit this field. A
    /// server with authentication enabled rejects gated operations until the
    /// session is authenticated, either by this field or by a later `AUTH` frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
}

/// Server handshake acknowledgement payload (`HELLO_ACK`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloAck {
    /// The negotiated protocol version (min of client and server max).
    pub protocol_version: u8,
    /// The server's advertised capabilities.
    pub capabilities: ServerCapabilities,
    /// Whether the server requires authentication before gated operations.
    #[serde(default)]
    pub auth_required: bool,
    /// Whether this connection is already authenticated (for example because a
    /// valid `auth_token` was supplied in the handshake).
    #[serde(default)]
    pub authenticated: bool,
}

/// Client authentication payload (`AUTH`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthRequest {
    /// The static authentication token to verify.
    pub token: String,
}

/// Server authentication result payload (`AUTH_RESULT`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthResult {
    /// Whether the connection is now authenticated.
    pub authenticated: bool,
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
    /// MVCC health and pressure summary. Additive in AWP 0.3.1: older clients
    /// that do not model this field ignore it, and a server that omits it (the
    /// field defaults to `None`) stays compatible with newer clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mvcc: Option<MvccHealth>,
    /// Cluster / replication summary. Additive in AWP for AuraDB 0.4.0: present
    /// only when cluster mode is enabled, and ignored by older clients. The wire
    /// protocol version is unchanged — this is a purely additive JSON field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<ClusterHealth>,
}

/// Cluster and replication summary carried in [`HealthReport`].
///
/// Reported only when cluster mode is enabled. Every field is honest: a
/// single-node cluster reports `single_node = true` and zero peers rather than
/// implying replication that is not happening.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterHealth {
    /// Whether cluster (Raft) mode is enabled.
    pub enabled: bool,
    /// This node's id (hex), if initialized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// The cluster id (hex), if initialized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<String>,
    /// The consensus role (`leader` / `follower` / `candidate`).
    pub role: String,
    /// The current Raft term.
    pub term: u64,
    /// The recognized leader's id (hex), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader_id: Option<String>,
    /// The highest committed log index.
    pub commit_index: u64,
    /// The highest applied log index.
    pub applied_index: u64,
    /// The last log index on this node.
    pub last_log_index: u64,
    /// Configured peer count (0 for a single-node cluster).
    pub peer_count: usize,
    /// Whether this is a single-node cluster.
    pub single_node: bool,
    /// Replication lag in entries (committed minus applied).
    pub replication_lag_entries: u64,
    /// Whether the experimental multi-node preview is active on this node.
    /// Additive field (v0.5.0); older clients ignore it.
    #[serde(default)]
    pub preview_multi_node: bool,
    /// Whether a quorum is currently reachable from this node (multi-node only).
    #[serde(default)]
    pub quorum_available: bool,
    /// Per-peer reachability and replication state (multi-node preview only).
    /// Empty for single-node clusters and older servers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<ClusterPeerHealth>,
}

/// Per-peer reachability and replication state, carried in [`ClusterHealth`] for
/// the multi-node preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterPeerHealth {
    /// The peer's node id (hex).
    pub node_id: String,
    /// The peer's configured cluster transport address.
    pub addr: String,
    /// Whether this node currently holds an outbound connection to the peer.
    pub connected: bool,
    /// The leader's record of the peer's highest matching log index, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_index: Option<u64>,
    /// The leader's next index to send to the peer, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_index: Option<u64>,
}

/// MVCC health and version-pressure summary carried in [`HealthReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MvccHealth {
    /// Transactions currently holding a pinned snapshot.
    pub active_transactions: usize,
    /// Registered transactions that have timed out but not yet been cleaned up.
    pub timed_out_transactions: usize,
    /// The oldest read timestamp pinned by an active transaction, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_active_read_ts: Option<u64>,
    /// Age in seconds of the oldest active transaction, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_transaction_age_secs: Option<u64>,
    /// Total stored MVCC versions retained (including superseded and tombstones).
    pub retained_versions: usize,
    /// Cumulative transactions reaped for exceeding the idle timeout.
    pub transaction_timeouts_total: u64,
    /// Configured transaction idle timeout in seconds (`0` = disabled).
    pub transaction_timeout_secs: u64,
    /// Whether background version GC is enabled.
    pub gc_enabled: bool,
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
            auth_required: true,
            authenticated: false,
        };
        let frame = Frame::json(Opcode::HelloAck, RequestId::ZERO, 0, &ack).unwrap();
        let back: HelloAck = frame.decode_json().unwrap();
        assert_eq!(back, ack);
    }

    #[test]
    fn hello_request_without_token_omits_field() {
        let req = HelloRequest {
            client_version: "test".into(),
            protocol_version: 1,
            auth_token: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("auth_token"));
        let back: HelloRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn auth_messages_roundtrip() {
        let req = AuthRequest {
            token: "secret".into(),
        };
        let frame = Frame::json(Opcode::Auth, RequestId::ZERO, 0, &req).unwrap();
        let back: AuthRequest = frame.decode_json().unwrap();
        assert_eq!(back, req);

        let res = AuthResult {
            authenticated: true,
        };
        let frame = Frame::json(Opcode::AuthResult, RequestId::ZERO, 0, &res).unwrap();
        let back: AuthResult = frame.decode_json().unwrap();
        assert_eq!(back, res);
    }
}
