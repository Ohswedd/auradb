//! The single-node cluster coordinator wired into the server.
//!
//! [`ClusterNode`] is what the server constructs when `cluster.enabled = true`
//! and no peers are configured: a real, durable, single-node Raft deployment.
//! Every data-plane write is proposed to the Raft log, committed (trivially, as
//! the sole voter), and applied to the engine; on restart, committed-but-unapplied
//! entries are replayed. The same code paths the multi-node tests exercise run
//! here — there is simply one voter.
//!
//! Multi-node *server* deployment is intentionally **not** wired up in v0.4.0:
//! the consensus core and apply path are validated through the deterministic
//! in-memory tests, but cross-process transport and its security story are not
//! production-ready, so the server fails closed when peers are configured (see
//! [`ClusterNode::bootstrap`]).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use auradb::{Engine, ReplicatedLog};
use auradb_cluster::{ClusterConfig, ClusterIdentity, ClusterStatus, NodeRole};
use auradb_raft::{
    Command, FileStorage, LogIndex, RaftConfig, RaftError, RaftMetrics, RaftNode, RaftStorage,
};
use auradb_storage::Batch;

use crate::apply::apply_command;
use crate::command::ReplicatedCommand;
use crate::error::{ReplicationError, Result};

/// A live snapshot of replication counters for observability.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplicationMetrics {
    /// Number of times this node became leader.
    pub leader_changes: u64,
    /// Number of votes granted by this node.
    pub votes_granted: u64,
    /// AppendEntries messages sent.
    pub append_entries_sent: u64,
    /// AppendEntries messages received.
    pub append_entries_received: u64,
    /// Committed-minus-applied lag in entries.
    pub replication_lag_entries: u64,
    /// Errors encountered while applying committed entries.
    pub apply_errors: u64,
}

/// A durable, single-node Raft coordinator.
pub struct ClusterNode {
    raft: Arc<Mutex<RaftNode<FileStorage>>>,
    engine: Engine,
    identity: ClusterIdentity,
    config: ClusterConfig,
    apply_errors: Arc<AtomicU64>,
    /// Offset added to a Raft log index to produce the MVCC commit timestamp.
    ///
    /// A fresh cluster uses `0` (commit ts == log index). When cluster mode is
    /// enabled on a data directory that already has committed MVCC versions, the
    /// base is pinned to that pre-existing commit watermark so the first cluster
    /// write's timestamp still exceeds it (timestamps must be strictly
    /// increasing). It is persisted so restarts reuse the same mapping.
    commit_ts_base: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CommitBase {
    commit_ts_base: u64,
}

const COMMIT_BASE_FILE: &str = "commit-base.json";

impl ClusterNode {
    /// Bootstrap (or recover) a single-node cluster.
    ///
    /// Fails closed with [`ReplicationError`]'s `Raft` path is not used here;
    /// instead, a multi-node configuration is rejected by the caller. This
    /// constructor assumes a single-voter deployment: it elects this node leader
    /// and replays any committed-but-unapplied entries into the engine.
    pub fn bootstrap(
        engine: Engine,
        identity: ClusterIdentity,
        config: ClusterConfig,
        raft_dir: impl AsRef<std::path::Path>,
    ) -> Result<ClusterNode> {
        let raft_dir = raft_dir.as_ref().to_path_buf();
        let storage = FileStorage::open(&raft_dir)?;
        // Pin (or load) the commit-timestamp base before applying anything: on a
        // fresh data directory this is 0, on upgraded data with existing MVCC
        // versions it is that watermark so cluster writes stay strictly newer.
        let commit_ts_base = load_or_init_commit_base(&raft_dir, &engine)?;
        let mut node = RaftNode::new(RaftConfig::single_node(identity.node_id()), storage);
        // A single voter is its own majority: campaigning elects us immediately.
        node.campaign();
        // The election no-op is committed; drain it so the applied pointer tracks
        // the commit index. It applies to nothing.
        let _ = node.take_committed();
        let cluster = ClusterNode {
            raft: Arc::new(Mutex::new(node)),
            engine,
            identity,
            config,
            apply_errors: Arc::new(AtomicU64::new(0)),
            commit_ts_base,
        };
        cluster.recover()?;
        Ok(cluster)
    }

    /// Replay committed data entries the engine has not yet applied. This closes
    /// the crash window between a durable Raft commit and the storage apply.
    fn recover(&self) -> Result<()> {
        let node = self.raft.lock().expect("raft mutex");
        let commit = node.commit_index().get();
        // Replay every committed entry whose derived commit timestamp the engine
        // has not yet reached. Apply is idempotent below the watermark.
        let mut idx = 1u64;
        while idx <= commit {
            if let Some(entry) = node.storage().entry_at(LogIndex(idx)) {
                let command = ReplicatedCommand::decode(&entry.command)?;
                let commit_ts = self.commit_ts_base + idx;
                if let Err(e) = apply_command(&self.engine, &command, commit_ts) {
                    self.apply_errors.fetch_add(1, Ordering::Relaxed);
                    return Err(e);
                }
            }
            idx += 1;
        }
        Ok(())
    }

    /// A handle the engine can use as its replicated write log.
    pub fn write_log(&self) -> Arc<dyn ReplicatedLog> {
        Arc::new(RaftWriteLog {
            raft: Arc::clone(&self.raft),
            commit_ts_base: self.commit_ts_base,
        })
    }

    /// This node's identity.
    pub fn identity(&self) -> &ClusterIdentity {
        &self.identity
    }

    /// A point-in-time cluster status snapshot.
    pub fn status(&self) -> ClusterStatus {
        let node = self.raft.lock().expect("raft mutex");
        ClusterStatus {
            enabled: true,
            node_id: Some(self.identity.node_id()),
            cluster_id: Some(self.identity.cluster_id()),
            role: node.role(),
            term: node.term().get(),
            leader_id: node.leader_id(),
            commit_index: node.commit_index().get(),
            applied_index: node.applied_index().get(),
            last_log_index: node.last_log_index().get(),
            peer_count: self.config.peers.len(),
            single_node: self.config.peers.is_empty(),
        }
    }

    /// Whether this node currently accepts writes (it is the leader).
    pub fn is_leader(&self) -> bool {
        self.raft.lock().expect("raft mutex").role() == NodeRole::Leader
    }

    /// A snapshot of replication metrics.
    pub fn metrics(&self) -> ReplicationMetrics {
        let node = self.raft.lock().expect("raft mutex");
        let raft: &RaftMetrics = node.metrics();
        ReplicationMetrics {
            leader_changes: raft.leader_changes,
            votes_granted: raft.votes_granted,
            append_entries_sent: raft.append_entries_sent,
            append_entries_received: raft.append_entries_received,
            replication_lag_entries: node.replication_lag(),
            apply_errors: self.apply_errors.load(Ordering::Relaxed),
        }
    }
}

/// The `ReplicatedLog` the engine commits through. It only touches Raft — the
/// engine applies the batch to storage inline once `replicate` returns, so there
/// is no double apply.
struct RaftWriteLog {
    raft: Arc<Mutex<RaftNode<FileStorage>>>,
    commit_ts_base: u64,
}

impl ReplicatedLog for RaftWriteLog {
    fn replicate(&self, batch: &Batch) -> auradb_core::Result<u64> {
        let mut node = self.raft.lock().expect("raft mutex");
        if node.role() != NodeRole::Leader {
            return Err(auradb_core::Error::NotLeader(not_leader_hint(
                node.leader_id(),
            )));
        }
        let command: Command = ReplicatedCommand::Write(batch.clone())
            .encode()
            .map_err(repl_err_to_core)?;
        let index = node.propose(command).map_err(raft_err_to_core)?;
        // Single voter: the entry is already committed. Advance the applied
        // pointer; the engine applies the batch to storage inline. The commit
        // timestamp is the log index offset by the pinned base.
        let _ = node.take_committed();
        Ok(self.commit_ts_base + index.get())
    }
}

/// Load the persisted commit-timestamp base, or pin it to the engine's current
/// commit watermark on first enable and persist it. A fresh data directory pins
/// `0` (commit ts == log index); a directory with pre-existing MVCC versions
/// pins their watermark so cluster writes stay strictly newer.
fn load_or_init_commit_base(raft_dir: &std::path::Path, engine: &Engine) -> Result<u64> {
    let path = raft_dir.join(COMMIT_BASE_FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let base: CommitBase = serde_json::from_str(&text)
            .map_err(|e| ReplicationError::Codec(format!("commit base: {e}")))?;
        return Ok(base.commit_ts_base);
    }
    let base = engine.commit_watermark();
    std::fs::create_dir_all(raft_dir).map_err(|e| ReplicationError::Codec(e.to_string()))?;
    let text = serde_json::to_string_pretty(&CommitBase {
        commit_ts_base: base,
    })
    .map_err(|e| ReplicationError::Codec(e.to_string()))?;
    std::fs::write(&path, text).map_err(|e| ReplicationError::Codec(e.to_string()))?;
    Ok(base)
}

fn not_leader_hint(leader: Option<auradb_cluster::NodeId>) -> String {
    match leader {
        Some(id) => format!("this node is not the leader; current leader is node {id}"),
        None => "this node is not the leader and no leader is currently known".to_string(),
    }
}

fn raft_err_to_core(e: RaftError) -> auradb_core::Error {
    match e {
        RaftError::NotLeader(hint) => auradb_core::Error::NotLeader(not_leader_hint(hint)),
        other => auradb_core::Error::Internal(format!("raft: {other}")),
    }
}

fn repl_err_to_core(e: ReplicationError) -> auradb_core::Error {
    match e {
        ReplicationError::Apply(err) => err,
        other => auradb_core::Error::Internal(format!("replication: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn follower_write_is_rejected_as_not_leader() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::open(dir.path()).unwrap();
        // A node that has not campaigned is a follower and must refuse writes.
        let node = RaftNode::new(
            RaftConfig::single_node(auradb_cluster::NodeId::from_raw(1)),
            storage,
        );
        let log = RaftWriteLog {
            raft: Arc::new(Mutex::new(node)),
            commit_ts_base: 0,
        };
        let batch = Batch {
            txn_id: auradb_core::TxnId(1),
            ops: vec![],
        };
        let err = log.replicate(&batch).unwrap_err();
        assert_eq!(err.code(), auradb_core::ErrorCode::NotLeader);
    }
}
