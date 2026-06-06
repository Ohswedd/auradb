//! Cross-process peer transport for the experimental multi-node preview.
//!
//! This module carries Raft messages between AuraDB server processes over a
//! dedicated cluster socket. It is deliberately small and explicit:
//!
//! - every frame is magic-tagged, version-tagged, length-delimited, and
//!   CRC32-checksummed, with a payload-length limit;
//! - a connection opens with a [`PeerMessage::Hello`] handshake that verifies the
//!   protocol version, the cluster id, the peer's node id (against the static
//!   membership), and a shared authentication token;
//! - unknown, duplicate, or wrong-cluster peers are rejected with a structured
//!   [`PeerError`];
//! - snapshot install is carried as a bounded, single-message
//!   [`PeerMessage::InstallSnapshotRequest`] (the preview transfers a whole
//!   snapshot in one frame, capped by [`MAX_SNAPSHOT_BYTES`]) and answered with a
//!   [`PeerMessage::InstallSnapshotResponse`]; an unrecognized request is still
//!   answered with a structured [`PeerMessage::Unsupported`] rather than silently
//!   ignored;
//! - secrets (the peer auth token) never appear in `Debug` output.
//!
//! Loopback-only deployments may run without TLS (the documented preview
//! default). Non-loopback deployments require TLS plus the auth token; the
//! configuration layer (`auradb-cluster`) fails closed otherwise.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;

use auradb_cluster::{ClusterId, ClusterTlsConfig, NodeId, Secret};
use auradb_raft::Message as RaftMessage;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Frame magic: "Aura Peer Raft, v1".
pub const PEER_MAGIC: [u8; 4] = *b"APR1";
/// The peer wire protocol version.
pub const PROTOCOL_VERSION: u8 = 1;
/// Maximum accepted peer frame payload (16 MiB). Oversized frames are rejected
/// before allocation.
pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;
/// Maximum accepted single-message snapshot payload (8 MiB), strictly below
/// [`MAX_FRAME_BYTES`] so the encoded request still fits one frame. The preview
/// ships a whole snapshot in one bounded message rather than chunked streaming;
/// a snapshot larger than this is refused on both the send and receive sides.
pub const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

/// A stream usable for the peer transport (plain TCP or TLS).
pub trait PeerIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> PeerIo for T {}

/// A boxed peer stream (plain or TLS), used uniformly after the connection is
/// established.
pub type PeerStream = Box<dyn PeerIo>;

/// Errors raised by the peer transport.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Underlying I/O failure.
    #[error("peer transport io error: {0}")]
    Io(#[from] io::Error),
    /// The frame magic did not match: the peer is not speaking the protocol.
    #[error("peer frame magic mismatch: not an AuraDB peer connection")]
    BadMagic,
    /// The frame declared an unsupported protocol version.
    #[error("peer frame protocol version {found} is not supported (expected {expected})")]
    BadVersion {
        /// The version the peer sent.
        found: u8,
        /// The version this build speaks.
        expected: u8,
    },
    /// The frame payload exceeded the configured limit.
    #[error("peer frame payload {len} bytes exceeds limit {limit} bytes")]
    Oversized {
        /// The declared payload length.
        len: u32,
        /// The configured limit.
        limit: u32,
    },
    /// The frame checksum did not match its payload.
    #[error("peer frame checksum mismatch (corrupt or tampered frame)")]
    BadChecksum,
    /// The frame payload was not valid JSON for a [`PeerMessage`].
    #[error("peer frame decode error: {0}")]
    Decode(String),
    /// The peer rejected the handshake (or vice versa).
    #[error("peer handshake rejected: {0}")]
    Rejected(PeerError),
    /// The handshake did not begin with a `Hello`.
    #[error("peer did not open with a Hello handshake")]
    NoHello,
}

impl From<TransportError> for crate::error::ReplicationError {
    fn from(e: TransportError) -> Self {
        crate::error::ReplicationError::Transport(e.to_string())
    }
}

/// A structured peer-protocol error sent on the wire and surfaced to operators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum PeerError {
    /// The presented cluster id does not match this node's cluster.
    #[error("cluster id mismatch")]
    ClusterIdMismatch,
    /// The presented node id is not a configured static member.
    #[error("unknown node id (not a configured static peer)")]
    UnknownNode,
    /// A connection from this node id is already established.
    #[error("duplicate node connection")]
    DuplicateNode,
    /// The peer authentication token was missing or incorrect.
    #[error("peer authentication failed")]
    AuthFailed,
    /// The requested operation is not supported by this build.
    #[error("unsupported peer operation: {0}")]
    Unsupported(String),
    /// A generic, human-readable rejection reason.
    #[error("{0}")]
    Other(String),
}

/// The peer handshake greeting. Carries the sender's identity and the shared
/// authentication token (redacted in `Debug`).
#[derive(Clone, Serialize, Deserialize)]
pub struct Hello {
    /// The sender's cluster id.
    pub cluster_id: ClusterId,
    /// The sender's node id.
    pub node_id: NodeId,
    /// The sender's advertised cluster address.
    pub advertise_addr: String,
    /// The shared peer authentication token (empty if none configured).
    pub token: Secret,
}

impl fmt::Debug for Hello {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hello")
            .field("cluster_id", &self.cluster_id)
            .field("node_id", &self.node_id)
            .field("advertise_addr", &self.advertise_addr)
            .field("token", &self.token) // Secret redacts itself.
            .finish()
    }
}

/// The handshake acknowledgement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloAck {
    /// The responder's node id.
    pub node_id: NodeId,
}

/// A message on the peer wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerMessage {
    /// Connection greeting.
    Hello(Hello),
    /// Greeting acknowledgement (handshake accepted).
    HelloAck(HelloAck),
    /// A Raft RPC from `from`.
    Raft {
        /// The sending node id.
        from: NodeId,
        /// The Raft message body (RequestVote, AppendEntries, and responses).
        message: RaftMessage,
    },
    /// A leader installs a bounded, single-message state-machine snapshot on a
    /// follower that has fallen behind the leader's compacted log prefix. The
    /// snapshot is carried as an encoded [`crate::SnapshotManifest`] in
    /// `snapshot`, bounded by [`MAX_SNAPSHOT_BYTES`].
    InstallSnapshotRequest {
        /// The sending (leader) node id.
        from: NodeId,
        /// The leader's term.
        term: u64,
        /// The last log index the snapshot includes.
        last_included_index: u64,
        /// The term of `last_included_index`.
        last_included_term: u64,
        /// The encoded snapshot manifest (schemas, records, metadata, digest).
        /// Base64-encoded on the wire so it stays one compact JSON string rather
        /// than a byte-array that would bloat past the frame limit.
        #[serde(with = "base64_bytes")]
        snapshot: Vec<u8>,
    },
    /// A follower's response to an [`PeerMessage::InstallSnapshotRequest`].
    InstallSnapshotResponse {
        /// The responding (follower) node id.
        from: NodeId,
        /// The follower's term, so the leader steps down on a higher term.
        term: u64,
        /// Whether the snapshot was validated and installed.
        success: bool,
        /// On success, the boundary index the follower installed (echoed back).
        last_included_index: u64,
    },
    /// A structured "not supported" response.
    Unsupported {
        /// What was requested.
        request: String,
    },
    /// A structured error (typically a handshake rejection).
    Error(PeerError),
}

/// Compact, dependency-free base64 (standard alphabet) for the snapshot payload
/// so an `InstallSnapshotRequest` serializes as one JSON string rather than a
/// byte-array that would multiply its size past the frame limit.
mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        decode(&s).map_err(serde::de::Error::custom)
    }

    fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
            out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[((n >> 6) & 63) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        out
    }

    fn decode(s: &str) -> Result<Vec<u8>, String> {
        fn val(c: u8) -> Result<u32, String> {
            match c {
                b'A'..=b'Z' => Ok((c - b'A') as u32),
                b'a'..=b'z' => Ok((c - b'a' + 26) as u32),
                b'0'..=b'9' => Ok((c - b'0' + 52) as u32),
                b'+' => Ok(62),
                b'/' => Ok(63),
                _ => Err(format!("invalid base64 byte {c:#x}")),
            }
        }
        let s = s.trim().as_bytes();
        if s.len() % 4 != 0 {
            return Err("base64 length must be a multiple of 4".into());
        }
        let mut out = Vec::with_capacity(s.len() / 4 * 3);
        for chunk in s.chunks(4) {
            let pad = chunk.iter().filter(|&&c| c == b'=').count();
            let mut n = 0u32;
            for &c in chunk {
                n = (n << 6) | if c == b'=' { 0 } else { val(c)? };
            }
            out.push((n >> 16) as u8);
            if pad < 2 {
                out.push((n >> 8) as u8);
            }
            if pad < 1 {
                out.push(n as u8);
            }
        }
        Ok(out)
    }
}

/// The static membership view the transport validates against: the set of
/// configured peer node ids plus this node's own cluster id.
#[derive(Debug, Clone)]
pub struct Membership {
    /// This node's cluster id.
    pub cluster_id: ClusterId,
    /// Configured static peer node ids (excludes this node).
    pub peer_ids: Vec<NodeId>,
    /// The shared peer auth token (empty if none required).
    pub token: Secret,
}

impl Membership {
    /// Whether `id` is a configured static peer.
    pub fn knows(&self, id: NodeId) -> bool {
        self.peer_ids.contains(&id)
    }
}

/// Validate an inbound `Hello` against the local membership and the set of
/// already-connected peer node ids. Returns the peer's node id on success or a
/// structured [`PeerError`] on rejection. This is the security choke point for
/// inbound peer connections and is unit-tested directly (no sockets required).
pub fn validate_hello(
    hello: &Hello,
    membership: &Membership,
    connected: &HashMap<NodeId, ()>,
) -> std::result::Result<NodeId, PeerError> {
    if hello.cluster_id != membership.cluster_id {
        return Err(PeerError::ClusterIdMismatch);
    }
    if !membership.knows(hello.node_id) {
        return Err(PeerError::UnknownNode);
    }
    // Constant-ish token comparison. An empty configured token means "no token
    // required" (loopback preview); otherwise the presented token must match.
    if !membership.token.is_empty() && hello.token.expose() != membership.token.expose() {
        return Err(PeerError::AuthFailed);
    }
    if connected.contains_key(&hello.node_id) {
        return Err(PeerError::DuplicateNode);
    }
    Ok(hello.node_id)
}

/// Write one framed [`PeerMessage`].
pub async fn write_message<W>(w: &mut W, msg: &PeerMessage) -> Result<(), TransportError>
where
    W: AsyncWrite + Unpin,
{
    let payload = serde_json::to_vec(msg).map_err(|e| TransportError::Decode(e.to_string()))?;
    let len = payload.len() as u32;
    let crc = crc32fast::hash(&payload);
    let mut header = [0u8; 13];
    header[0..4].copy_from_slice(&PEER_MAGIC);
    header[4] = PROTOCOL_VERSION;
    header[5..9].copy_from_slice(&len.to_be_bytes());
    header[9..13].copy_from_slice(&crc.to_be_bytes());
    w.write_all(&header).await?;
    w.write_all(&payload).await?;
    w.flush().await?;
    Ok(())
}

/// Read one framed [`PeerMessage`], enforcing the magic, version, size limit,
/// and checksum.
pub async fn read_message<R>(r: &mut R, max_bytes: u32) -> Result<PeerMessage, TransportError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; 13];
    r.read_exact(&mut header).await?;
    if header[0..4] != PEER_MAGIC {
        return Err(TransportError::BadMagic);
    }
    let version = header[4];
    if version != PROTOCOL_VERSION {
        return Err(TransportError::BadVersion {
            found: version,
            expected: PROTOCOL_VERSION,
        });
    }
    let len = u32::from_be_bytes([header[5], header[6], header[7], header[8]]);
    if len > max_bytes {
        return Err(TransportError::Oversized {
            len,
            limit: max_bytes,
        });
    }
    let crc = u32::from_be_bytes([header[9], header[10], header[11], header[12]]);
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).await?;
    if crc32fast::hash(&payload) != crc {
        return Err(TransportError::BadChecksum);
    }
    serde_json::from_slice(&payload).map_err(|e| TransportError::Decode(e.to_string()))
}

// ----- TLS material for the peer transport -----

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn load_certs(
    path: &Path,
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, TransportError> {
    let bytes = std::fs::read(path)?;
    let certs = rustls_pemfile::certs(&mut &bytes[..])
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| TransportError::Decode(format!("peer cert {}: {e}", path.display())))?;
    if certs.is_empty() {
        return Err(TransportError::Decode(format!(
            "no certificates in {}",
            path.display()
        )));
    }
    Ok(certs)
}

fn load_key(path: &Path) -> Result<rustls::pki_types::PrivateKeyDer<'static>, TransportError> {
    let bytes = std::fs::read(path)?;
    rustls_pemfile::private_key(&mut &bytes[..])
        .map_err(|e| TransportError::Decode(format!("peer key {}: {e}", path.display())))?
        .ok_or_else(|| TransportError::Decode(format!("no private key in {}", path.display())))
}

fn root_store(path: &Path) -> Result<rustls::RootCertStore, TransportError> {
    let mut roots = rustls::RootCertStore::empty();
    for ca in load_certs(path)? {
        roots
            .add(ca)
            .map_err(|e| TransportError::Decode(format!("peer CA {}: {e}", path.display())))?;
    }
    Ok(roots)
}

/// Build a mutual-TLS acceptor for inbound peer connections from validated peer
/// TLS material. Peers must present a certificate trusted by `ca_path`.
pub fn build_peer_acceptor(
    tls: &ClusterTlsConfig,
) -> Result<tokio_rustls::TlsAcceptor, TransportError> {
    let cert = tls
        .cert_path
        .as_ref()
        .ok_or_else(|| TransportError::Decode("cluster.tls.cert_path required".into()))?;
    let key = tls
        .key_path
        .as_ref()
        .ok_or_else(|| TransportError::Decode("cluster.tls.key_path required".into()))?;
    let ca = tls
        .ca_path
        .as_ref()
        .ok_or_else(|| TransportError::Decode("cluster.tls.ca_path required".into()))?;
    let certs = load_certs(cert)?;
    let key = load_key(key)?;
    let roots = root_store(ca)?;
    let verifier =
        rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider())
            .build()
            .map_err(|e| TransportError::Decode(format!("peer client verifier: {e}")))?;
    let config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| TransportError::Decode(format!("peer TLS: {e}")))?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)
        .map_err(|e| TransportError::Decode(format!("peer server cert: {e}")))?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// Build a mutual-TLS connector for outbound peer connections. This node
/// presents its own certificate and verifies peers against `ca_path`.
pub fn build_peer_connector(
    tls: &ClusterTlsConfig,
) -> Result<tokio_rustls::TlsConnector, TransportError> {
    let cert = tls
        .cert_path
        .as_ref()
        .ok_or_else(|| TransportError::Decode("cluster.tls.cert_path required".into()))?;
    let key = tls
        .key_path
        .as_ref()
        .ok_or_else(|| TransportError::Decode("cluster.tls.key_path required".into()))?;
    let ca = tls
        .ca_path
        .as_ref()
        .ok_or_else(|| TransportError::Decode("cluster.tls.ca_path required".into()))?;
    let certs = load_certs(cert)?;
    let key = load_key(key)?;
    let roots = root_store(ca)?;
    let config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| TransportError::Decode(format!("peer TLS: {e}")))?
        .with_root_certificates(roots)
        .with_client_auth_cert(certs, key)
        .map_err(|e| TransportError::Decode(format!("peer client cert: {e}")))?;
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cid(v: u128) -> ClusterId {
        ClusterId::new(v).unwrap()
    }

    fn membership(token: &str) -> Membership {
        Membership {
            cluster_id: cid(0xABCD),
            peer_ids: vec![NodeId::from_raw(2), NodeId::from_raw(3)],
            token: Secret::new(token),
        }
    }

    fn hello(cluster: u128, node: u64, token: &str) -> Hello {
        Hello {
            cluster_id: cid(cluster),
            node_id: NodeId::from_raw(node),
            advertise_addr: "127.0.0.1:7272".into(),
            token: Secret::new(token),
        }
    }

    #[tokio::test]
    async fn frame_round_trips() {
        let msg = PeerMessage::HelloAck(HelloAck {
            node_id: NodeId::from_raw(7),
        });
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let got = read_message(&mut cursor, MAX_FRAME_BYTES).await.unwrap();
        assert!(matches!(got, PeerMessage::HelloAck(ack) if ack.node_id == NodeId::from_raw(7)));
    }

    #[tokio::test]
    async fn peer_transport_rejects_oversized_frame() {
        let msg = PeerMessage::Unsupported {
            request: "x".repeat(100),
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        // A tiny limit forces the oversize check to fire.
        let err = read_message(&mut cursor, 8).await.unwrap_err();
        assert!(matches!(err, TransportError::Oversized { .. }), "{err}");
    }

    #[tokio::test]
    async fn peer_transport_rejects_bad_version() {
        let msg = PeerMessage::HelloAck(HelloAck {
            node_id: NodeId::from_raw(1),
        });
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        // Corrupt the version byte (index 4).
        buf[4] = 99;
        let mut cursor = std::io::Cursor::new(buf);
        let err = read_message(&mut cursor, MAX_FRAME_BYTES)
            .await
            .unwrap_err();
        assert!(
            matches!(err, TransportError::BadVersion { found: 99, .. }),
            "{err}"
        );
    }

    #[tokio::test]
    async fn peer_transport_rejects_bad_magic() {
        let msg = PeerMessage::HelloAck(HelloAck {
            node_id: NodeId::from_raw(1),
        });
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        buf[0] = b'X';
        let mut cursor = std::io::Cursor::new(buf);
        let err = read_message(&mut cursor, MAX_FRAME_BYTES)
            .await
            .unwrap_err();
        assert!(matches!(err, TransportError::BadMagic), "{err}");
    }

    #[tokio::test]
    async fn peer_transport_rejects_corrupt_checksum() {
        let msg = PeerMessage::HelloAck(HelloAck {
            node_id: NodeId::from_raw(1),
        });
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        // Flip a payload byte (past the 13-byte header).
        let last = buf.len() - 1;
        buf[last] ^= 0xFF;
        let mut cursor = std::io::Cursor::new(buf);
        let err = read_message(&mut cursor, MAX_FRAME_BYTES)
            .await
            .unwrap_err();
        assert!(matches!(err, TransportError::BadChecksum), "{err}");
    }

    #[test]
    fn peer_hello_rejects_wrong_cluster_id() {
        let m = membership("");
        let connected = HashMap::new();
        let err = validate_hello(&hello(0x1111, 2, ""), &m, &connected).unwrap_err();
        assert_eq!(err, PeerError::ClusterIdMismatch);
    }

    #[test]
    fn peer_hello_rejects_unknown_node() {
        let m = membership("");
        let connected = HashMap::new();
        // Node 9 is not a configured peer.
        let err = validate_hello(&hello(0xABCD, 9, ""), &m, &connected).unwrap_err();
        assert_eq!(err, PeerError::UnknownNode);
    }

    #[test]
    fn peer_hello_rejects_duplicate_node() {
        let m = membership("");
        let mut connected = HashMap::new();
        connected.insert(NodeId::from_raw(2), ());
        let err = validate_hello(&hello(0xABCD, 2, ""), &m, &connected).unwrap_err();
        assert_eq!(err, PeerError::DuplicateNode);
    }

    #[test]
    fn peer_hello_rejects_bad_token() {
        let m = membership("the-secret");
        let connected = HashMap::new();
        let err = validate_hello(&hello(0xABCD, 2, "wrong"), &m, &connected).unwrap_err();
        assert_eq!(err, PeerError::AuthFailed);
    }

    #[test]
    fn peer_hello_accepts_valid() {
        let m = membership("the-secret");
        let connected = HashMap::new();
        let id = validate_hello(&hello(0xABCD, 2, "the-secret"), &m, &connected).unwrap();
        assert_eq!(id, NodeId::from_raw(2));
    }

    #[test]
    fn peer_transport_redacts_secrets() {
        let h = hello(0xABCD, 2, "super-secret-token");
        let rendered = format!("{h:?}");
        assert!(
            !rendered.contains("super-secret-token"),
            "Hello debug leaked the token: {rendered}"
        );
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }
}
