//! The server's handle to whichever cluster mode is active.
//!
//! Cluster mode comes in two flavors that share one status/metrics surface:
//!
//! - [`ClusterRuntime::Single`] — a durable single-node Raft deployment (the
//!   v0.4.x path; the recommended production cluster mode);
//! - [`ClusterRuntime::Multi`] — the experimental cross-process multi-node
//!   preview, enabled only with `experimental_multi_node = true` and a static
//!   peer set.
//!
//! The server holds one of these behind `ServerContext.cluster` and treats them
//! uniformly for status, health, and metrics.

use std::sync::Arc;

use auradb::ReplicatedLog;
use auradb_cluster::{ClusterIdentity, ClusterStatus};
use auradb_replication::{ClusterNode, PeerCluster, PeerMetrics, PeerStatus, ReplicationMetrics};

/// The active cluster runtime: single-node or the multi-node preview.
pub enum ClusterRuntime {
    /// A durable single-node Raft deployment.
    Single(Arc<ClusterNode>),
    /// An experimental cross-process multi-node preview node.
    Multi(Arc<PeerCluster>),
}

impl ClusterRuntime {
    /// A point-in-time cluster status snapshot.
    pub fn status(&self) -> ClusterStatus {
        match self {
            ClusterRuntime::Single(n) => n.status(),
            ClusterRuntime::Multi(n) => n.status(),
        }
    }

    /// A snapshot of replication metrics.
    pub fn metrics(&self) -> ReplicationMetrics {
        match self {
            ClusterRuntime::Single(n) => n.metrics(),
            ClusterRuntime::Multi(n) => n.metrics(),
        }
    }

    /// Whether this node currently accepts writes (it is the leader).
    pub fn is_leader(&self) -> bool {
        match self {
            ClusterRuntime::Single(n) => n.is_leader(),
            ClusterRuntime::Multi(n) => n.is_leader(),
        }
    }

    /// The replicated write log the engine commits through.
    pub fn write_log(&self) -> Arc<dyn ReplicatedLog> {
        match self {
            ClusterRuntime::Single(n) => n.write_log(),
            ClusterRuntime::Multi(n) => n.write_log(),
        }
    }

    /// This node's identity.
    pub fn identity(&self) -> &ClusterIdentity {
        match self {
            ClusterRuntime::Single(n) => n.identity(),
            ClusterRuntime::Multi(n) => n.identity(),
        }
    }

    /// Whether this is the multi-node preview.
    pub fn is_multi_node(&self) -> bool {
        matches!(self, ClusterRuntime::Multi(_))
    }

    /// Per-peer reachability and replication state (empty for single-node).
    pub fn peer_status(&self) -> Vec<PeerStatus> {
        match self {
            ClusterRuntime::Single(_) => Vec::new(),
            ClusterRuntime::Multi(n) => n.peer_status(),
        }
    }

    /// Detailed peer/Raft counters (None for single-node).
    pub fn peer_metrics(&self) -> Option<PeerMetrics> {
        match self {
            ClusterRuntime::Single(_) => None,
            ClusterRuntime::Multi(n) => Some(n.peer_metrics()),
        }
    }

    /// Whether a quorum is currently reachable (always true for single-node).
    pub fn quorum_available(&self) -> bool {
        match self {
            ClusterRuntime::Single(_) => true,
            ClusterRuntime::Multi(n) => n.quorum_available(),
        }
    }

    /// Stop any background networking (multi-node only) and wait for it to end.
    pub async fn shutdown(&self) {
        if let ClusterRuntime::Multi(n) = self {
            n.shutdown().await;
        }
    }
}
