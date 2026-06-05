//! # auradb-cluster
//!
//! Cluster identity, metadata, configuration, and role state for AuraDB.
//!
//! This crate owns the *durable, single-node-observable* facts about a node's
//! place in a cluster: its [`NodeId`], the [`ClusterId`] it belongs to, the
//! parsed [`ClusterConfig`], the current [`NodeRole`], and a [`ClusterStatus`]
//! snapshot suitable for `auradb status --json` and health reports.
//!
//! It contains **no** consensus logic and **no** networking — those live in
//! `auradb-raft` and the server. Keeping identity separate means the CLI and
//! diagnostics can inspect a data directory without standing up a Raft node.
//!
//! When cluster mode is disabled, none of this affects engine behavior; the
//! `[cluster]` config table is inert and the single-node path is unchanged.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod config;
mod error;
mod id;
mod metadata;
mod role;

pub use config::{ClusterConfig, ClusterTlsConfig, PeerConfig, Secret, DEFAULT_CLUSTER_ADDR};
pub use error::{ClusterError, Result};
pub use id::{ClusterId, NodeId};
pub use metadata::{
    ClusterIdentity, ClusterMetadata, ClusterStore, NodeMetadata, METADATA_FORMAT_VERSION,
};
pub use role::NodeRole;

use serde::{Deserialize, Serialize};

/// A point-in-time snapshot of a node's cluster state.
///
/// This is the shape surfaced by `auradb status --json`, `auradb cluster
/// status`, and the server health report. Every field is honest: when cluster
/// mode is disabled, `enabled` is `false` and the consensus fields are `None`
/// or zero rather than implying a cluster that does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterStatus {
    /// Whether cluster (Raft) mode is enabled on this node.
    pub enabled: bool,
    /// This node's id, if initialized.
    pub node_id: Option<NodeId>,
    /// The cluster id, if initialized.
    pub cluster_id: Option<ClusterId>,
    /// The current consensus role.
    pub role: NodeRole,
    /// The current Raft term.
    pub term: u64,
    /// The id of the leader this node currently recognizes, if any.
    pub leader_id: Option<NodeId>,
    /// The highest committed log index.
    pub commit_index: u64,
    /// The highest applied (to the engine) log index.
    pub applied_index: u64,
    /// The last log index present on this node.
    pub last_log_index: u64,
    /// The number of configured peers (0 for a single-node cluster).
    pub peer_count: usize,
    /// Whether this is a single-node cluster (no peers).
    pub single_node: bool,
}

impl ClusterStatus {
    /// The status of a node with cluster mode disabled (the default path).
    pub fn disabled() -> ClusterStatus {
        ClusterStatus {
            enabled: false,
            node_id: None,
            cluster_id: None,
            role: NodeRole::Follower,
            term: 0,
            leader_id: None,
            commit_index: 0,
            applied_index: 0,
            last_log_index: 0,
            peer_count: 0,
            single_node: false,
        }
    }

    /// A status for an initialized but idle (not-yet-running) single node, used
    /// by offline CLI commands that inspect a data directory.
    pub fn idle_single_node(identity: &ClusterIdentity) -> ClusterStatus {
        ClusterStatus {
            enabled: true,
            node_id: Some(identity.node_id()),
            cluster_id: Some(identity.cluster_id()),
            role: NodeRole::Follower,
            term: 0,
            leader_id: None,
            commit_index: 0,
            applied_index: 0,
            last_log_index: 0,
            peer_count: 0,
            single_node: true,
        }
    }

    /// Whether this node currently accepts writes (it is the leader).
    pub fn accepts_writes(&self) -> bool {
        !self.enabled || self.role.accepts_writes()
    }

    /// Replication lag in log entries (committed minus applied).
    pub fn replication_lag_entries(&self) -> u64 {
        self.commit_index.saturating_sub(self.applied_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_status_accepts_writes() {
        let s = ClusterStatus::disabled();
        assert!(!s.enabled);
        assert!(s.accepts_writes());
        assert_eq!(s.replication_lag_entries(), 0);
    }

    #[test]
    fn lag_is_committed_minus_applied() {
        let mut s = ClusterStatus::disabled();
        s.commit_index = 10;
        s.applied_index = 7;
        assert_eq!(s.replication_lag_entries(), 3);
    }
}
