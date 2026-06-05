//! Cluster configuration as parsed from the server config file.
//!
//! This mirrors the `[cluster]` table of `AuraDB.toml`. It is deliberately a
//! plain, serde-friendly struct so the server config crate can embed it. All
//! semantic validation lives in [`ClusterConfig::validate`]; the server calls it
//! at startup and `auradb cluster doctor` calls it offline.
//!
//! v0.5.0 introduces the controlled, experimental multi-node preview. Forming a
//! real cross-process cluster requires two explicit opt-ins
//! (`enabled = true` and `experimental_multi_node = true`) and fails closed on
//! any non-loopback peer address unless `allow_experimental_public_cluster` is
//! also set, which in turn requires peer TLS and a peer authentication token.
//! Single-node mode remains the recommended production path.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{ClusterError, Result};

/// Default cluster listen/advertise address (loopback, dedicated cluster port).
pub const DEFAULT_CLUSTER_ADDR: &str = "127.0.0.1:7172";

/// A secret string (peer authentication token) that never reveals itself in
/// `Debug` output. It is stored and serialized transparently so it round-trips
/// through the config file, but logging a [`ClusterConfig`] never leaks it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Build a secret from a raw string.
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }
    /// Whether the secret is empty (unset).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Borrow the underlying secret. Callers must not log the result.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            f.write_str("Secret(unset)")
        } else {
            f.write_str("Secret(<redacted>)")
        }
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Secret {
        Secret(value.to_string())
    }
}

/// A single static cluster member: its stable node id and its peer address.
///
/// Membership is static for the preview — there is no join or leave. Every node
/// lists every other node here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerConfig {
    /// The peer's stable node id (hex).
    pub node_id: String,
    /// The peer's cluster transport address (`host:port`).
    pub addr: String,
    /// The peer's client-facing address (`host:port`), if known. Optional and
    /// additive: when an operator declares it, a `not_leader` response and the
    /// cluster diagnostics can report the leader's *client* address so a client
    /// can redirect. When unset, the leader's client address is reported as
    /// unknown rather than guessed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_addr: Option<String>,
}

/// TLS material for the peer (cluster) transport.
///
/// This is independent of the client-facing `[tls]` block: a deployment can
/// terminate client TLS and peer TLS with different certificates. Peer TLS is
/// required whenever the cluster transport is exposed beyond loopback.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClusterTlsConfig {
    /// Whether the peer transport uses TLS.
    pub enabled: bool,
    /// Path to the PEM-encoded peer server certificate chain.
    pub cert_path: Option<PathBuf>,
    /// Path to the PEM-encoded peer private key (PKCS#8 or RSA/SEC1).
    pub key_path: Option<PathBuf>,
    /// Path to a PEM-encoded CA bundle used to verify peer certificates.
    pub ca_path: Option<PathBuf>,
}

/// The `[cluster]` configuration table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClusterConfig {
    /// Whether cluster (Raft) mode is enabled. Defaults to `false`, in which
    /// case AuraDB behaves exactly as the single-node engine.
    pub enabled: bool,
    /// Whether the experimental cross-process multi-node preview is enabled.
    /// Required (in addition to `enabled`) before a non-empty `peers` list is
    /// accepted. Without it, any configured peer fails closed at startup.
    pub experimental_multi_node: bool,
    /// Whether the cluster transport may bind or advertise non-loopback
    /// addresses. Off by default: a non-loopback peer address fails closed
    /// unless this is set, and setting it additionally requires peer TLS and a
    /// peer authentication token.
    pub allow_experimental_public_cluster: bool,
    /// Optional pinned cluster id (hex). Empty means "use the persisted id, or
    /// generate one on bootstrap".
    pub cluster_id: String,
    /// Optional pinned node id (hex). Empty means "use the persisted id, or
    /// generate one on init".
    pub node_id: String,
    /// Address the cluster (Raft) transport listens on.
    pub listen_addr: String,
    /// Address advertised to peers (may differ from `listen_addr` behind NAT).
    pub advertise_addr: String,
    /// Whether this node bootstraps a brand-new cluster (initiates the first
    /// election). For the static preview every node may bootstrap.
    pub bootstrap: bool,
    /// Static peer set for the multi-node preview. Each entry names a peer's
    /// node id and address. Empty means single-node cluster.
    pub peers: Vec<PeerConfig>,
    /// Shared peer authentication token. Presented in the peer handshake and
    /// compared on both ends; required for public-cluster mode.
    pub peer_auth_token: Secret,
    /// Peer transport TLS configuration.
    pub tls: ClusterTlsConfig,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        ClusterConfig {
            enabled: false,
            experimental_multi_node: false,
            allow_experimental_public_cluster: false,
            cluster_id: String::new(),
            node_id: String::new(),
            listen_addr: DEFAULT_CLUSTER_ADDR.to_string(),
            advertise_addr: DEFAULT_CLUSTER_ADDR.to_string(),
            bootstrap: true,
            peers: Vec::new(),
            peer_auth_token: Secret::default(),
            tls: ClusterTlsConfig::default(),
        }
    }
}

impl ClusterConfig {
    /// A single-node cluster configuration that bootstraps on the default
    /// address.
    pub fn single_node() -> Self {
        ClusterConfig {
            enabled: true,
            ..ClusterConfig::default()
        }
    }

    /// Whether this configuration describes a multi-node deployment (peers set).
    pub fn is_multi_node(&self) -> bool {
        self.enabled && !self.peers.is_empty()
    }

    /// Whether the cluster listen address is a loopback-only bind.
    pub fn is_loopback(&self) -> bool {
        host_of(&self.listen_addr)
            .map(is_loopback_host)
            .unwrap_or(false)
    }

    /// Whether any cluster address (listen, advertise, or a peer) is
    /// non-loopback, i.e. whether this is a "public" cluster deployment.
    pub fn is_public(&self) -> bool {
        let non_loopback = |addr: &str| host_of(addr).map(is_loopback_host) != Some(true);
        non_loopback(&self.listen_addr)
            || non_loopback(&self.advertise_addr)
            || self.peers.iter().any(|p| non_loopback(&p.addr))
    }

    /// Validate the configuration, returning a typed error on any problem.
    ///
    /// Validation is a no-op when `enabled` is `false`: a disabled cluster
    /// section never affects single-node behavior.
    pub fn validate(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        validate_addr("listen_addr", &self.listen_addr)?;
        validate_addr("advertise_addr", &self.advertise_addr)?;
        let own_node = if self.node_id.is_empty() {
            None
        } else {
            Some(
                self.node_id
                    .parse::<crate::NodeId>()
                    .map_err(|e| ClusterError::Config(e.to_string()))?,
            )
        };
        if !self.cluster_id.is_empty() {
            self.cluster_id
                .parse::<crate::ClusterId>()
                .map_err(|e| ClusterError::Config(e.to_string()))?;
        }

        // The multi-node preview is gated behind an explicit second opt-in. A
        // configured peer set without it fails closed (preserves v0.4.1).
        if !self.peers.is_empty() && !self.experimental_multi_node {
            return Err(ClusterError::Config(
                "cluster peers are configured but experimental_multi_node is false: the \
                 cross-process multi-node preview must be enabled explicitly with \
                 experimental_multi_node = true. Single-node mode (no peers) remains the \
                 recommended production path; see docs/CLUSTERING.md"
                    .into(),
            ));
        }

        let mut seen_addr: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut seen_node: std::collections::HashSet<crate::NodeId> =
            std::collections::HashSet::new();
        for peer in &self.peers {
            validate_addr("peer addr", &peer.addr)?;
            if let Some(client_addr) = &peer.client_addr {
                validate_addr("peer client_addr", client_addr)?;
            }
            if peer.node_id.is_empty() {
                return Err(ClusterError::Config(format!(
                    "peer {:?} is missing node_id: every static peer needs node_id and addr",
                    peer.addr
                )));
            }
            let peer_node = peer
                .node_id
                .parse::<crate::NodeId>()
                .map_err(|e| ClusterError::Config(format!("peer node_id {e}")))?;
            let norm = peer.addr.trim();
            if !seen_addr.insert(norm) {
                return Err(ClusterError::Config(format!(
                    "duplicate peer address {:?}: each peer must be listed once",
                    peer.addr
                )));
            }
            if !seen_node.insert(peer_node) {
                return Err(ClusterError::Config(format!(
                    "duplicate peer node_id {:?}: each peer must be listed once",
                    peer.node_id
                )));
            }
            // A peer that points back at this node's own address or id is a
            // configuration error: a node is never its own peer.
            if norm == self.listen_addr.trim() || norm == self.advertise_addr.trim() {
                return Err(ClusterError::Config(format!(
                    "peer address {:?} is this node's own address; a node cannot peer with itself",
                    peer.addr
                )));
            }
            if Some(peer_node) == own_node {
                return Err(ClusterError::Config(format!(
                    "peer node_id {:?} is this node's own id; a node cannot peer with itself",
                    peer.node_id
                )));
            }
        }

        if !self.bootstrap && self.peers.is_empty() {
            return Err(ClusterError::Config(
                "cluster is enabled with bootstrap = false but no peers are configured: a joining \
                 node needs at least one peer to contact"
                    .into(),
            ));
        }

        // Public-cluster guardrails: any non-loopback address requires the
        // explicit opt-in, and that opt-in requires TLS plus a peer auth token.
        if self.is_public() {
            if !self.allow_experimental_public_cluster {
                return Err(ClusterError::Config(
                    "cluster transport binds, advertises, or peers a non-loopback address but \
                     allow_experimental_public_cluster is false: loopback-only peer networking is \
                     the default for the preview. Set allow_experimental_public_cluster = true to \
                     accept the risk (TLS and a peer auth token are then required)."
                        .into(),
                ));
            }
            if !self.tls.enabled {
                return Err(ClusterError::Config(
                    "public cluster mode requires peer TLS: set [cluster.tls] enabled = true with \
                     cert_path, key_path, and ca_path"
                        .into(),
                ));
            }
            if self.peer_auth_token.is_empty() {
                return Err(ClusterError::Config(
                    "public cluster mode requires a peer authentication token: set \
                     [cluster] peer_auth_token"
                        .into(),
                ));
            }
        }

        // If peer TLS is enabled, its material must be fully specified.
        if self.tls.enabled
            && (self.tls.cert_path.is_none()
                || self.tls.key_path.is_none()
                || self.tls.ca_path.is_none())
        {
            return Err(ClusterError::Config(
                "[cluster.tls] enabled requires cert_path, key_path, and ca_path".into(),
            ));
        }
        Ok(())
    }
}

fn validate_addr(field: &str, addr: &str) -> Result<()> {
    let host = host_of(addr)
        .ok_or_else(|| ClusterError::Config(format!("{field} must be host:port, got {addr:?}")))?;
    let port = addr.rsplit(':').next().unwrap_or("");
    if host.is_empty() {
        return Err(ClusterError::Config(format!(
            "{field} has an empty host: {addr:?}"
        )));
    }
    port.parse::<u16>()
        .map_err(|_| ClusterError::Config(format!("{field} has an invalid port: {addr:?}")))?;
    Ok(())
}

fn host_of(addr: &str) -> Option<&str> {
    addr.rsplit_once(':').map(|(h, _)| h)
}

fn is_loopback_host(host: &str) -> bool {
    host == "127.0.0.1" || host == "::1" || host == "localhost" || host == "[::1]"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(node_id: &str, addr: &str) -> PeerConfig {
        PeerConfig {
            node_id: node_id.to_string(),
            addr: addr.to_string(),
            client_addr: None,
        }
    }

    #[test]
    fn disabled_config_validates_trivially() {
        let cfg = ClusterConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn single_node_is_loopback_by_default() {
        let cfg = ClusterConfig::single_node();
        assert!(cfg.is_loopback());
        assert!(!cfg.is_multi_node());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn invalid_addr_is_rejected() {
        let mut cfg = ClusterConfig::single_node();
        cfg.listen_addr = "not-an-addr".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn join_without_peers_is_rejected() {
        let mut cfg = ClusterConfig::single_node();
        cfg.bootstrap = false;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn multi_node_requires_explicit_preview_flag() {
        let mut cfg = ClusterConfig::single_node();
        cfg.peers = vec![peer("00000000000000a2", "127.0.0.1:7272")];
        // experimental_multi_node is false: fail closed.
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("experimental_multi_node"), "{err}");
        // With the explicit opt-in, a loopback peer set is accepted.
        cfg.experimental_multi_node = true;
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        assert!(cfg.is_multi_node());
    }

    #[test]
    fn non_loopback_peer_rejected_without_public_preview_flag() {
        let mut cfg = ClusterConfig::single_node();
        cfg.experimental_multi_node = true;
        cfg.peers = vec![peer("00000000000000a2", "10.0.0.2:7272")];
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("allow_experimental_public_cluster"),
            "{err}"
        );
    }

    #[test]
    fn public_cluster_requires_tls_and_peer_auth() {
        let mut cfg = ClusterConfig::single_node();
        cfg.experimental_multi_node = true;
        cfg.allow_experimental_public_cluster = true;
        cfg.listen_addr = "10.0.0.1:7272".into();
        cfg.advertise_addr = "10.0.0.1:7272".into();
        cfg.peers = vec![peer("00000000000000a2", "10.0.0.2:7272")];
        // No TLS: rejected.
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("peer TLS"), "{err}");

        // TLS enabled but no token: rejected.
        cfg.tls = ClusterTlsConfig {
            enabled: true,
            cert_path: Some("cert.pem".into()),
            key_path: Some("key.pem".into()),
            ca_path: Some("ca.pem".into()),
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("peer authentication token"),
            "{err}"
        );

        // TLS plus token: accepted.
        cfg.peer_auth_token = Secret::new("a-shared-secret");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn static_membership_only() {
        // Membership is exactly the configured peer set: no field exists to add
        // or remove members at runtime, and the configured peers validate as-is.
        let mut cfg = ClusterConfig::single_node();
        cfg.experimental_multi_node = true;
        cfg.peers = vec![
            peer("00000000000000a2", "127.0.0.1:7272"),
            peer("00000000000000a3", "127.0.0.1:7372"),
        ];
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        assert_eq!(cfg.peers.len(), 2);
    }

    #[test]
    fn invalid_peer_config_fails_closed() {
        // Bad address.
        let mut cfg = ClusterConfig::single_node();
        cfg.experimental_multi_node = true;
        cfg.peers = vec![peer("00000000000000a2", "not-an-addr")];
        assert!(cfg.validate().is_err());

        // Missing node id.
        cfg.peers = vec![peer("", "127.0.0.1:7272")];
        assert!(cfg.validate().is_err());

        // Duplicate address.
        cfg.peers = vec![
            peer("00000000000000a2", "127.0.0.1:7272"),
            peer("00000000000000a3", "127.0.0.1:7272"),
        ];
        assert!(cfg.validate().is_err());

        // Duplicate node id.
        cfg.peers = vec![
            peer("00000000000000a2", "127.0.0.1:7272"),
            peer("00000000000000a2", "127.0.0.1:7372"),
        ];
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn self_referential_peer_is_rejected() {
        let mut cfg = ClusterConfig::single_node();
        cfg.experimental_multi_node = true;
        cfg.listen_addr = "127.0.0.1:7172".into();
        cfg.advertise_addr = "127.0.0.1:7172".into();
        cfg.peers = vec![peer("00000000000000a2", "127.0.0.1:7172")];
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("own address"), "{err}");
    }

    #[test]
    fn self_node_id_peer_is_rejected() {
        let mut cfg = ClusterConfig::single_node();
        cfg.experimental_multi_node = true;
        cfg.node_id = "00000000000000a1".into();
        cfg.peers = vec![peer("00000000000000a1", "127.0.0.1:7272")];
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("own id"), "{err}");
    }

    #[test]
    fn secret_redacts_in_debug() {
        let s = Secret::new("super-secret-token");
        assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
        assert!(!format!("{s:?}").contains("super-secret"));
        assert!(Secret::default().is_empty());
    }
}
