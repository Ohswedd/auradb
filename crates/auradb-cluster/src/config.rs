//! Cluster configuration as parsed from the server config file.
//!
//! This mirrors the `[cluster]` table of `AuraDB.toml`. It is deliberately a
//! plain, serde-friendly struct so the server config crate can embed it. All
//! semantic validation lives in [`ClusterConfig::validate`]; the server calls it
//! at startup and `auradb cluster doctor` calls it offline.

use serde::{Deserialize, Serialize};

use crate::error::{ClusterError, Result};

/// Default cluster listen/advertise address (loopback, dedicated cluster port).
pub const DEFAULT_CLUSTER_ADDR: &str = "127.0.0.1:7172";

/// The `[cluster]` configuration table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClusterConfig {
    /// Whether cluster (Raft) mode is enabled. Defaults to `false`, in which
    /// case AuraDB behaves exactly as the single-node engine.
    pub enabled: bool,
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
    /// Whether this node bootstraps a brand-new single-node cluster.
    pub bootstrap: bool,
    /// Peer cluster addresses for multi-node deployments.
    pub peers: Vec<String>,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        ClusterConfig {
            enabled: false,
            cluster_id: String::new(),
            node_id: String::new(),
            listen_addr: DEFAULT_CLUSTER_ADDR.to_string(),
            advertise_addr: DEFAULT_CLUSTER_ADDR.to_string(),
            bootstrap: true,
            peers: Vec::new(),
        }
    }
}

impl ClusterConfig {
    /// A single-node cluster configuration that bootstraps on the given address.
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
        if !self.node_id.is_empty() {
            self.node_id
                .parse::<crate::NodeId>()
                .map_err(|e| ClusterError::Config(e.to_string()))?;
        }
        if !self.cluster_id.is_empty() {
            self.cluster_id
                .parse::<crate::ClusterId>()
                .map_err(|e| ClusterError::Config(e.to_string()))?;
        }
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for peer in &self.peers {
            validate_addr("peer", peer)?;
            let norm = peer.trim();
            if !seen.insert(norm) {
                return Err(ClusterError::Config(format!(
                    "duplicate peer address {peer:?}: each peer must be listed once"
                )));
            }
            // A peer that points back at this node's own listen/advertise address
            // is a configuration error: a node is never its own peer.
            if norm == self.listen_addr.trim() || norm == self.advertise_addr.trim() {
                return Err(ClusterError::Config(format!(
                    "peer address {peer:?} is this node's own address; a node cannot peer with \
                     itself"
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
    fn peers_make_it_multi_node() {
        let mut cfg = ClusterConfig::single_node();
        cfg.peers = vec!["10.0.0.2:7172".into()];
        assert!(cfg.is_multi_node());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn bad_node_id_is_rejected() {
        let mut cfg = ClusterConfig::single_node();
        cfg.node_id = "nothex".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn duplicate_peers_are_rejected() {
        let mut cfg = ClusterConfig::single_node();
        cfg.peers = vec!["10.0.0.2:7172".into(), "10.0.0.2:7172".into()];
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("duplicate peer"), "{err}");
    }

    #[test]
    fn self_referential_peer_is_rejected() {
        let mut cfg = ClusterConfig::single_node();
        cfg.listen_addr = "10.0.0.1:7172".into();
        cfg.advertise_addr = "10.0.0.1:7172".into();
        cfg.peers = vec!["10.0.0.1:7172".into()];
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("own address"), "{err}");
    }

    #[test]
    fn invalid_peer_address_is_rejected() {
        let mut cfg = ClusterConfig::single_node();
        cfg.peers = vec!["not-an-addr".into()];
        assert!(cfg.validate().is_err());
    }
}
