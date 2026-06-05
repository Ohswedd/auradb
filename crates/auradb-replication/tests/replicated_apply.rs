//! End-to-end replication tests: single-node durable cluster mode, the
//! multi-node replicated apply path (deterministic, in-process), and the
//! snapshot boundary.

use std::collections::{HashMap, VecDeque};

use auradb::core::{CollectionId, CollectionSchema, Document, FieldDef, FieldType, Record, Value};
use auradb::query::{FindQuery, Mutation};
use auradb::Engine;
use auradb_cluster::{ClusterConfig, ClusterStore, NodeId, NodeRole};
use auradb_raft::{
    Command, Envelope, FileStorage, HardState, LogEntry, LogIndex, MemStorage, RaftConfig,
    RaftNode, RaftStorage, Term,
};
use auradb_replication::{
    apply_command, ClusterNode, ReplicatedCommand, RestoreOptions, SchemaCommand, SnapshotManifest,
};
use auradb_storage::{Batch, LogOp};
use tempfile::{tempdir, TempDir};

// ---- helpers ----

fn schema() -> CollectionSchema {
    CollectionSchema::new("C")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Int,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("v", FieldType::Int))
}

fn record(id: i64, v: i64) -> Record {
    let mut fields = Document::new();
    fields.insert("id".into(), Value::Int(id));
    fields.insert("v".into(), Value::Int(v));
    Record::new(RecordId128(id), CollectionId::new("C"), fields)
}

#[allow(non_snake_case)]
fn RecordId128(id: i64) -> auradb::core::RecordId {
    auradb::core::RecordId::from_u128(id as u128)
}

fn write_batch(id: i64, v: i64) -> Batch {
    Batch {
        txn_id: auradb::core::TxnId(id as u64),
        ops: vec![LogOp::Put {
            commit_ts: 0,
            record: record(id, v),
        }],
    }
}

fn open_engine_with_schema() -> (Engine, TempDir) {
    let dir = tempdir().unwrap();
    let engine = Engine::open(dir.path().join("data")).unwrap();
    engine.create_schema(schema()).unwrap();
    (engine, dir)
}

// ---- single-node cluster mode ----

#[test]
fn single_node_cluster_write_commits_through_raft() {
    let dir = tempdir().unwrap();
    let engine = Engine::open(dir.path().join("data")).unwrap();
    engine.create_schema(schema()).unwrap();
    let identity = ClusterStore::new(dir.path().join("data"))
        .init(None, None, "0.4.0")
        .unwrap();
    let cn = ClusterNode::bootstrap(
        engine.clone(),
        identity,
        ClusterConfig::single_node(),
        dir.path().join("data").join("cluster"),
    )
    .unwrap();
    engine.attach_replicated_log(cn.write_log());

    // A normal write now flows through the Raft log.
    engine
        .apply_mutation(Mutation::Insert {
            collection: "C".into(),
            fields: {
                let mut f = Document::new();
                f.insert("id".into(), Value::Int(1));
                f.insert("v".into(), Value::Int(10));
                f
            },
        })
        .unwrap();

    let rows = engine.find(&FindQuery::new("C")).unwrap();
    assert_eq!(rows.len(), 1);
    let status = cn.status();
    assert!(status.enabled);
    assert_eq!(status.role, NodeRole::Leader);
    assert!(
        status.commit_index >= 2,
        "election no-op + the write are committed"
    );
    assert!(status.last_log_index >= 2);
}

#[test]
fn schema_command_applies_once() {
    let (engine, _dir) = {
        let dir = tempdir().unwrap();
        (Engine::open(dir.path().join("data")).unwrap(), dir)
    };
    let cmd = ReplicatedCommand::Schema(SchemaCommand::Create(Box::new(schema())));
    apply_command(&engine, &cmd, 1).unwrap();
    assert!(engine.get_schema("C").is_some());
    // Applying again is idempotent (replace), not an error.
    apply_command(&engine, &cmd, 2).unwrap();
    assert_eq!(engine.list_schemas().len(), 1);
}

#[test]
fn record_command_applies_once() {
    let (engine, _dir) = open_engine_with_schema();
    let cmd = ReplicatedCommand::Write(write_batch(1, 10));
    apply_command(&engine, &cmd, 5).unwrap();
    assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 1);
    // Replaying the same committed index is a no-op (idempotent).
    apply_command(&engine, &cmd, 5).unwrap();
    let rows = engine.find(&FindQuery::new("C")).unwrap();
    assert_eq!(rows.len(), 1, "no duplicate record from replay");
}

#[test]
fn transaction_batch_applies_atomically() {
    let (engine, _dir) = open_engine_with_schema();
    // A multi-op batch is one committed unit at one log index / commit ts.
    let batch = Batch {
        txn_id: auradb::core::TxnId(7),
        ops: vec![
            LogOp::Put {
                commit_ts: 0,
                record: record(1, 100),
            },
            LogOp::Put {
                commit_ts: 0,
                record: record(2, 200),
            },
        ],
    };
    apply_command(&engine, &ReplicatedCommand::Write(batch), 3).unwrap();
    let rows = engine.find(&FindQuery::new("C")).unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn restart_replays_committed_raft_entries() {
    let (engine, dir) = open_engine_with_schema();
    let raft_dir = dir.path().join("raftlog");
    // Hand-build a durable log with a committed write the engine never applied.
    {
        let mut s = FileStorage::open(&raft_dir).unwrap();
        s.append(&[
            LogEntry {
                term: Term(1),
                index: LogIndex(1),
                command: Command::noop(),
            },
            LogEntry {
                term: Term(1),
                index: LogIndex(2),
                command: ReplicatedCommand::Write(write_batch(1, 10))
                    .encode()
                    .unwrap(),
            },
        ])
        .unwrap();
        s.save_hard_state(&HardState {
            current_term: Term(1),
            voted_for: None,
            commit_index: LogIndex(2),
        })
        .unwrap();
    }
    assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 0);

    // The restart apply driver replays committed entries the engine is missing.
    let node = RaftNode::new(
        RaftConfig::single_node(NodeId::from_raw(1)),
        FileStorage::open(&raft_dir).unwrap(),
    );
    let applied = engine.commit_watermark();
    for idx in (applied + 1)..=node.commit_index().get() {
        if let Some(entry) = node.storage().entry_at(LogIndex(idx)) {
            let cmd = ReplicatedCommand::decode(&entry.command).unwrap();
            apply_command(&engine, &cmd, idx).unwrap();
        }
    }
    assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 1);
}

#[test]
fn uncommitted_entries_not_applied_after_restart() {
    let (engine, dir) = open_engine_with_schema();
    let raft_dir = dir.path().join("raftlog");
    // A write at index 2 exists in the log but was never committed (commit=1).
    {
        let mut s = FileStorage::open(&raft_dir).unwrap();
        s.append(&[
            LogEntry {
                term: Term(1),
                index: LogIndex(1),
                command: Command::noop(),
            },
            LogEntry {
                term: Term(1),
                index: LogIndex(2),
                command: ReplicatedCommand::Write(write_batch(1, 10))
                    .encode()
                    .unwrap(),
            },
        ])
        .unwrap();
        s.save_hard_state(&HardState {
            current_term: Term(1),
            voted_for: None,
            commit_index: LogIndex(1),
        })
        .unwrap();
    }
    let node = RaftNode::new(
        RaftConfig::single_node(NodeId::from_raw(1)),
        FileStorage::open(&raft_dir).unwrap(),
    );
    // The driver applies strictly up to the committed index.
    let applied = engine.commit_watermark();
    for idx in (applied + 1)..=node.commit_index().get() {
        if let Some(entry) = node.storage().entry_at(LogIndex(idx)) {
            let cmd = ReplicatedCommand::decode(&entry.command).unwrap();
            apply_command(&engine, &cmd, idx).unwrap();
        }
    }
    assert_eq!(
        engine.find(&FindQuery::new("C")).unwrap().len(),
        0,
        "the uncommitted write at index 2 is not applied"
    );
}

// ---- multi-node replicated apply (deterministic, in-process) ----

struct MultiNode {
    raft: RaftNode<MemStorage>,
    engine: Engine,
    _dir: TempDir,
}

struct ReplCluster {
    nodes: HashMap<NodeId, MultiNode>,
    order: Vec<NodeId>,
    bus: VecDeque<(NodeId, Envelope)>,
}

impl ReplCluster {
    fn new(ids: &[NodeId]) -> ReplCluster {
        let mut nodes = HashMap::new();
        for &id in ids {
            let dir = tempdir().unwrap();
            let engine = Engine::open(dir.path().join("data")).unwrap();
            engine.create_schema(schema()).unwrap();
            let cfg = RaftConfig {
                id,
                peers: ids.iter().copied().filter(|&p| p != id).collect(),
                election_timeout_min: 10,
                election_timeout_max: 20,
                heartbeat_interval: 3,
            };
            nodes.insert(
                id,
                MultiNode {
                    raft: RaftNode::new(cfg, MemStorage::new()),
                    engine,
                    _dir: dir,
                },
            );
        }
        ReplCluster {
            nodes,
            order: ids.to_vec(),
            bus: VecDeque::new(),
        }
    }

    fn pump(&mut self, id: NodeId) {
        let node = self.nodes.get_mut(&id).unwrap();
        for env in node.raft.take_messages() {
            self.bus.push_back((id, env));
        }
        // Apply any newly committed entries to this node's engine.
        let committed = node.raft.take_committed();
        for entry in committed {
            if let Ok(cmd) = ReplicatedCommand::decode(&entry.command) {
                let _ = apply_command(&node.engine, &cmd, entry.index.get());
            }
        }
    }

    fn tick(&mut self) {
        for id in self.order.clone() {
            self.nodes.get_mut(&id).unwrap().raft.tick();
            self.pump(id);
        }
    }

    fn deliver_all(&mut self) {
        let mut guard = 0;
        while let Some((from, env)) = self.bus.pop_front() {
            let to = env.to;
            if let Some(node) = self.nodes.get_mut(&to) {
                node.raft.step(from, env.message).unwrap();
            }
            self.pump(to);
            guard += 1;
            assert!(guard < 100_000, "delivery did not converge");
        }
    }

    fn leader(&self) -> Option<NodeId> {
        let ls: Vec<NodeId> = self
            .order
            .iter()
            .copied()
            .filter(|id| self.nodes[id].raft.role() == NodeRole::Leader)
            .collect();
        (ls.len() == 1).then(|| ls[0])
    }

    fn run_until_leader(&mut self) -> NodeId {
        for _ in 0..300 {
            self.tick();
            self.deliver_all();
            if let Some(l) = self.leader() {
                self.tick();
                self.deliver_all();
                return l;
            }
        }
        panic!("no leader elected");
    }

    fn propose_write(&mut self, leader: NodeId, batch: Batch) {
        let cmd = ReplicatedCommand::Write(batch).encode().unwrap();
        self.nodes
            .get_mut(&leader)
            .unwrap()
            .raft
            .propose(cmd)
            .unwrap();
        self.pump(leader);
        for _ in 0..10 {
            self.tick();
            self.deliver_all();
        }
    }
}

#[test]
fn follower_applies_leader_committed_entry() {
    let ids: Vec<NodeId> = (1..=3).map(NodeId::from_raw).collect();
    let mut cluster = ReplCluster::new(&ids);
    let leader = cluster.run_until_leader();
    cluster.propose_write(leader, write_batch(1, 42));
    for &id in &ids {
        let rows = cluster.nodes[&id]
            .engine
            .find(&FindQuery::new("C"))
            .unwrap();
        assert_eq!(rows.len(), 1, "node {id} applied the committed write");
    }
}

#[test]
fn mvcc_commit_order_preserved_through_raft() {
    let ids: Vec<NodeId> = (1..=3).map(NodeId::from_raw).collect();
    let mut cluster = ReplCluster::new(&ids);
    let leader = cluster.run_until_leader();
    // Three updates to the same record, in order.
    for v in [10, 20, 30] {
        cluster.propose_write(leader, write_batch(1, v));
    }
    for &id in &ids {
        let rows = cluster.nodes[&id]
            .engine
            .find(&FindQuery::new("C"))
            .unwrap();
        assert_eq!(rows.len(), 1);
        // The last write in log order wins on every replica.
        assert_eq!(rows[0].fields.get("v"), Some(&Value::Int(30)), "node {id}");
    }
}

#[test]
fn indexes_consistent_after_replicated_apply() {
    let ids: Vec<NodeId> = (1..=3).map(NodeId::from_raw).collect();
    let mut cluster = ReplCluster::new(&ids);
    let leader = cluster.run_until_leader();
    cluster.propose_write(leader, write_batch(1, 10));
    cluster.propose_write(leader, write_batch(2, 20));
    for &id in &ids {
        // A primary-key lookup uses the index; it must find the replicated row.
        let mut q = FindQuery::new("C");
        q.filter = Some(auradb::query::Filter::Compare {
            field: "id".into(),
            op: auradb::query::CompareOp::Eq,
            value: Value::Int(2),
        });
        let rows = cluster.nodes[&id].engine.find(&q).unwrap();
        assert_eq!(rows.len(), 1, "node {id} index resolves the replicated row");
        assert_eq!(rows[0].fields.get("v"), Some(&Value::Int(20)));
    }
}

#[test]
fn planner_stats_consistent_after_replicated_apply() {
    let ids: Vec<NodeId> = (1..=3).map(NodeId::from_raw).collect();
    let mut cluster = ReplCluster::new(&ids);
    let leader = cluster.run_until_leader();
    for id in 1..=4 {
        cluster.propose_write(leader, write_batch(id, id * 10));
    }
    for &id in &ids {
        // The engine's record count (kept current by the apply path's planner
        // stats refresh) matches on every replica.
        assert_eq!(cluster.nodes[&id].engine.stats().records, 4, "node {id}");
    }
}

// ---- snapshot boundary ----

fn populated_engine() -> (Engine, TempDir) {
    let (engine, dir) = open_engine_with_schema();
    for id in 1..=3 {
        engine
            .apply_mutation(Mutation::Insert {
                collection: "C".into(),
                fields: {
                    let mut f = Document::new();
                    f.insert("id".into(), Value::Int(id));
                    f.insert("v".into(), Value::Int(id * 10));
                    f
                },
            })
            .unwrap();
    }
    (engine, dir)
}

#[test]
fn snapshot_create_contains_manifest() {
    let (engine, _dir) = populated_engine();
    let snap = SnapshotManifest::create(&engine, 7, 2, "0.4.0").unwrap();
    assert_eq!(snap.meta.last_included_index, 7);
    assert_eq!(snap.meta.last_included_term, 2);
    assert!(!snap.payload.is_empty());
    assert!(snap.verified_payload().is_ok());
}

#[test]
fn snapshot_restore_rebuilds_state() {
    let (engine, _dir) = populated_engine();
    let snap = SnapshotManifest::create(&engine, 5, 1, "0.4.0").unwrap();
    let target = tempdir().unwrap();
    let restored = snap.restore(target.path().join("restored")).unwrap();
    assert_eq!(restored.find(&FindQuery::new("C")).unwrap().len(), 3);
}

#[test]
fn snapshot_restore_rebuilds_indexes() {
    let (engine, _dir) = populated_engine();
    let snap = SnapshotManifest::create(&engine, 5, 1, "0.4.0").unwrap();
    let target = tempdir().unwrap();
    let restored = snap.restore(target.path().join("restored")).unwrap();
    let mut q = FindQuery::new("C");
    q.filter = Some(auradb::query::Filter::Compare {
        field: "id".into(),
        op: auradb::query::CompareOp::Eq,
        value: Value::Int(2),
    });
    let rows = restored.find(&q).unwrap();
    assert_eq!(rows.len(), 1, "the restored primary-key index resolves");
}

#[test]
fn snapshot_restore_preserves_mvcc_latest_state() {
    let (engine, _dir) = open_engine_with_schema();
    // Insert then update so the latest committed value is what restore must keep.
    engine
        .apply_mutation(Mutation::Insert {
            collection: "C".into(),
            fields: {
                let mut f = Document::new();
                f.insert("id".into(), Value::Int(1));
                f.insert("v".into(), Value::Int(1));
                f
            },
        })
        .unwrap();
    engine
        .apply_mutation(Mutation::Upsert {
            collection: "C".into(),
            fields: {
                let mut f = Document::new();
                f.insert("id".into(), Value::Int(1));
                f.insert("v".into(), Value::Int(999));
                f
            },
        })
        .unwrap();
    let snap = SnapshotManifest::create(&engine, 9, 3, "0.4.0").unwrap();
    let target = tempdir().unwrap();
    let restored = snap.restore(target.path().join("restored")).unwrap();
    let rows = restored.find(&FindQuery::new("C")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields.get("v"), Some(&Value::Int(999)));
}

#[test]
fn snapshot_rejects_incompatible_version() {
    let (engine, _dir) = populated_engine();
    let mut snap = SnapshotManifest::create(&engine, 1, 1, "0.4.0").unwrap();
    snap.meta.format_version = auradb_replication::SNAPSHOT_FORMAT_VERSION + 1;
    let encoded = serde_json::to_vec(&snap).unwrap();
    assert!(SnapshotManifest::decode(&encoded).is_err());
    assert!(snap.verified_payload().is_err());
}

// ---- snapshot restore edge cases (v0.4.1) ----

#[test]
fn snapshot_restore_rejects_future_format() {
    let (engine, _dir) = populated_engine();
    let mut snap = SnapshotManifest::create(&engine, 1, 1, "0.4.1").unwrap();
    snap.meta.format_version = auradb_replication::SNAPSHOT_FORMAT_VERSION + 1;
    let target = tempdir().unwrap();
    let err = snap
        .restore_to(target.path().join("restored"), &RestoreOptions::default())
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(
        err,
        auradb_replication::ReplicationError::UnsupportedSnapshot { .. }
    ));
}

#[test]
fn snapshot_restore_rejects_cluster_mismatch() {
    let (engine, _dir) = populated_engine();
    let snap = SnapshotManifest::create(&engine, 1, 1, "0.4.1")
        .unwrap()
        .with_identity(Some("cafe0000".into()), Some("node01".into()));
    let target = tempdir().unwrap();
    let opts = RestoreOptions {
        expected_cluster_id: Some("beef0000".into()),
        ..RestoreOptions::default()
    };
    let err = snap
        .restore_to(target.path().join("restored"), &opts)
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(
        err,
        auradb_replication::ReplicationError::SnapshotRestoreRefused(_)
    ));
    // The same snapshot restores when the mismatch is explicitly allowed.
    let opts = RestoreOptions {
        expected_cluster_id: Some("beef0000".into()),
        allow_cluster_mismatch: true,
        ..RestoreOptions::default()
    };
    assert!(snap
        .restore_to(target.path().join("restored2"), &opts)
        .is_ok());
}

#[test]
fn snapshot_restore_rejects_corrupt_file() {
    let (engine, _dir) = populated_engine();
    let mut snap = SnapshotManifest::create(&engine, 1, 1, "0.4.1").unwrap();
    // Corrupt the payload so the digest no longer matches.
    snap.payload.push(0xff);
    let target = tempdir().unwrap();
    let err = snap
        .restore_to(target.path().join("restored"), &RestoreOptions::default())
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(
        err,
        auradb_replication::ReplicationError::SnapshotMalformed(_)
    ));
}

#[test]
fn snapshot_restore_rejects_nonempty_target_without_force() {
    let (engine, _dir) = populated_engine();
    let snap = SnapshotManifest::create(&engine, 1, 1, "0.4.1").unwrap();
    let target = tempdir().unwrap();
    let dest = target.path().join("restored");
    // First restore succeeds into an empty target.
    snap.restore_to(&dest, &RestoreOptions::default()).unwrap();
    // A second restore into the now-populated target is refused without force.
    let err = snap
        .restore_to(&dest, &RestoreOptions::default())
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(
        err,
        auradb_replication::ReplicationError::SnapshotRestoreRefused(_)
    ));
    // With force, it succeeds.
    assert!(snap
        .restore_to(
            &dest,
            &RestoreOptions {
                force: true,
                ..RestoreOptions::default()
            }
        )
        .is_ok());
}

#[test]
fn snapshot_restore_atomic_failure_preserves_existing_data() {
    // Populate a target directory through a real engine.
    let target = tempdir().unwrap();
    let dest = target.path().join("data");
    {
        let engine = Engine::open(&dest).unwrap();
        engine.create_schema(schema()).unwrap();
        engine
            .apply_mutation(Mutation::Insert {
                collection: "C".into(),
                fields: {
                    let mut f = Document::new();
                    f.insert("id".into(), Value::Int(7));
                    f.insert("v".into(), Value::Int(70));
                    f
                },
            })
            .unwrap();
    }
    // Build a snapshot, then corrupt its digest so restore must fail before it
    // ever destroys the existing target (validate-before-swap).
    let (src, _srcdir) = populated_engine();
    let mut snap = SnapshotManifest::create(&src, 1, 1, "0.4.1").unwrap();
    snap.payload.push(0x00);
    let err = snap
        .restore_to(
            &dest,
            &RestoreOptions {
                force: true,
                ..RestoreOptions::default()
            },
        )
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(
        err,
        auradb_replication::ReplicationError::SnapshotMalformed(_)
    ));
    // The original data is intact.
    let engine = Engine::open(&dest).unwrap();
    let rows = engine.find(&FindQuery::new("C")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields.get("v"), Some(&Value::Int(70)));
}

#[test]
fn snapshot_restore_rebuilds_indexes_and_stats() {
    let (engine, _dir) = populated_engine();
    let snap = SnapshotManifest::create(&engine, 5, 1, "0.4.1").unwrap();
    let target = tempdir().unwrap();
    let restored = snap
        .restore_to(target.path().join("restored"), &RestoreOptions::default())
        .unwrap();
    // Index lookup resolves, and planner stats reflect the restored rows.
    let mut q = FindQuery::new("C");
    q.filter = Some(auradb::query::Filter::Compare {
        field: "id".into(),
        op: auradb::query::CompareOp::Eq,
        value: Value::Int(2),
    });
    assert_eq!(restored.find(&q).unwrap().len(), 1);
    let stats = restored.planner_stats();
    let counted: usize = stats.collections.values().map(|c| c.row_count).sum();
    assert_eq!(counted, 3, "planner stats rebuilt for the restored rows");
}

#[test]
fn snapshot_restore_preserves_mvcc_latest_state_atomic() {
    let (engine, _dir) = open_engine_with_schema();
    engine
        .apply_mutation(Mutation::Insert {
            collection: "C".into(),
            fields: {
                let mut f = Document::new();
                f.insert("id".into(), Value::Int(1));
                f.insert("v".into(), Value::Int(1));
                f
            },
        })
        .unwrap();
    engine
        .apply_mutation(Mutation::Upsert {
            collection: "C".into(),
            fields: {
                let mut f = Document::new();
                f.insert("id".into(), Value::Int(1));
                f.insert("v".into(), Value::Int(999));
                f
            },
        })
        .unwrap();
    let snap = SnapshotManifest::create(&engine, 9, 3, "0.4.1").unwrap();
    let target = tempdir().unwrap();
    let restored = snap
        .restore_to(target.path().join("restored"), &RestoreOptions::default())
        .unwrap();
    let rows = restored.find(&FindQuery::new("C")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields.get("v"), Some(&Value::Int(999)));
}

#[test]
fn snapshot_inspect_reports_manifest() {
    let (engine, _dir) = populated_engine();
    let snap = SnapshotManifest::create(&engine, 12, 4, "0.4.1")
        .unwrap()
        .with_identity(Some("cafef00d".into()), Some("0000000000000001".into()));
    // Encode/decode round-trips the full manifest (what `snapshot inspect` reads).
    let bytes = snap.encode().unwrap();
    let back = SnapshotManifest::decode(&bytes).unwrap();
    assert_eq!(back.meta.last_included_index, 12);
    assert_eq!(back.meta.last_included_term, 4);
    assert_eq!(back.meta.collections, 1);
    assert_eq!(back.meta.records, 3);
    assert_eq!(back.meta.cluster_id.as_deref(), Some("cafef00d"));
    assert_eq!(
        back.meta.storage_format_version,
        auradb_storage::FORMAT_VERSION
    );
}

#[test]
fn snapshot_restore_preserves_cluster_metadata() {
    // A local cluster snapshot records the cluster/node identity it came from, so
    // an operator restoring it can confirm the cluster before re-initializing
    // identity on the restored directory.
    let (engine, _dir) = populated_engine();
    let snap = SnapshotManifest::create(&engine, 3, 1, "0.4.1")
        .unwrap()
        .with_identity(
            Some("0123456789abcdef".into()),
            Some("00000000000000aa".into()),
        );
    let target = tempdir().unwrap();
    let restored = snap
        .restore_to(target.path().join("restored"), &RestoreOptions::default())
        .unwrap();
    assert_eq!(restored.find(&FindQuery::new("C")).unwrap().len(), 3);
    assert_eq!(snap.meta.cluster_id.as_deref(), Some("0123456789abcdef"));
    assert_eq!(snap.meta.node_id.as_deref(), Some("00000000000000aa"));
}

// ---- compaction integration ----

#[test]
fn single_node_cluster_write_after_compaction() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    let engine = Engine::open(&data).unwrap();
    engine.create_schema(schema()).unwrap();
    let identity = ClusterStore::new(&data).init(None, None, "0.4.1").unwrap();
    let cn = ClusterNode::bootstrap(
        engine.clone(),
        identity,
        ClusterConfig::single_node(),
        data.join("cluster"),
    )
    .unwrap();
    engine.attach_replicated_log(cn.write_log());

    // A few writes through Raft.
    for id in 1..=3 {
        engine
            .apply_mutation(Mutation::Insert {
                collection: "C".into(),
                fields: {
                    let mut f = Document::new();
                    f.insert("id".into(), Value::Int(id));
                    f.insert("v".into(), Value::Int(id * 10));
                    f
                },
            })
            .unwrap();
    }
    // Compact the log up to the applied prefix.
    let report = cn.compact_log(false).unwrap();
    assert!(
        report.compacted,
        "applied prefix is compactable: {report:?}"
    );
    assert!(report.last_included_index >= 1);

    // Writes still work after compaction, and existing rows are intact.
    engine
        .apply_mutation(Mutation::Insert {
            collection: "C".into(),
            fields: {
                let mut f = Document::new();
                f.insert("id".into(), Value::Int(4));
                f.insert("v".into(), Value::Int(40));
                f
            },
        })
        .unwrap();
    let rows = engine.find(&FindQuery::new("C")).unwrap();
    assert_eq!(rows.len(), 4, "all rows present after compaction + write");
}

#[test]
fn snapshot_manifest_matches_compacted_prefix() {
    // A snapshot captured at the compaction boundary lines up with the log's
    // last included index/term, so the two agree on the covered prefix.
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    let engine = Engine::open(&data).unwrap();
    engine.create_schema(schema()).unwrap();
    let identity = ClusterStore::new(&data).init(None, None, "0.4.1").unwrap();
    let cluster_id = identity.cluster_id().to_string();
    let node_id = identity.node_id().to_string();
    let cn = ClusterNode::bootstrap(
        engine.clone(),
        identity,
        ClusterConfig::single_node(),
        data.join("cluster"),
    )
    .unwrap();
    engine.attach_replicated_log(cn.write_log());
    for id in 1..=3 {
        engine
            .apply_mutation(Mutation::Insert {
                collection: "C".into(),
                fields: {
                    let mut f = Document::new();
                    f.insert("id".into(), Value::Int(id));
                    f.insert("v".into(), Value::Int(id));
                    f
                },
            })
            .unwrap();
    }
    let dry = cn.compact_log(true).unwrap();
    // A snapshot taken at the dry-run boundary names the same prefix.
    let snap = SnapshotManifest::create(
        &engine,
        dry.last_included_index,
        dry.last_included_term,
        "0.4.1",
    )
    .unwrap()
    .with_identity(Some(cluster_id), Some(node_id));
    assert_eq!(snap.meta.last_included_index, dry.last_included_index);
    assert_eq!(snap.meta.last_included_term, dry.last_included_term);
    // Now really compact; the boundary matches the snapshot's last included index.
    let done = cn.compact_log(false).unwrap();
    assert_eq!(done.last_included_index, snap.meta.last_included_index);
}

// ---- apply idempotency under restart (v0.4.1) ----

#[test]
fn committed_entries_apply_once_after_restart() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    let raft = data.join("cluster");
    // First boot: write three records through the durable cluster.
    {
        let engine = Engine::open(&data).unwrap();
        engine.create_schema(schema()).unwrap();
        let identity = ClusterStore::new(&data).init(None, None, "0.4.1").unwrap();
        let cn = ClusterNode::bootstrap(
            engine.clone(),
            identity,
            ClusterConfig::single_node(),
            &raft,
        )
        .unwrap();
        engine.attach_replicated_log(cn.write_log());
        for id in 1..=3 {
            engine
                .apply_mutation(Mutation::Insert {
                    collection: "C".into(),
                    fields: {
                        let mut f = Document::new();
                        f.insert("id".into(), Value::Int(id));
                        f.insert("v".into(), Value::Int(id));
                        f
                    },
                })
                .unwrap();
        }
        assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 3);
    }
    // Reboot on the same directories: recovery replays committed entries but the
    // engine already applied them, so there are no duplicates.
    {
        let engine = Engine::open(&data).unwrap();
        let identity = ClusterStore::new(&data).load().unwrap().unwrap();
        let cn = ClusterNode::bootstrap(
            engine.clone(),
            identity,
            ClusterConfig::single_node(),
            &raft,
        )
        .unwrap();
        engine.attach_replicated_log(cn.write_log());
        assert_eq!(
            engine.find(&FindQuery::new("C")).unwrap().len(),
            3,
            "no duplicate records after restart replay"
        );
    }
}

#[test]
fn commit_before_apply_recovers_on_open() {
    // A crash after a Raft commit but before the engine applied it: bootstrap's
    // recovery replays the committed entry on open.
    let (engine, dir) = open_engine_with_schema();
    let data_dir = dir.path().join("data");
    let raft = data_dir.join("cluster");
    {
        let mut s = FileStorage::open(&raft).unwrap();
        s.append(&[
            LogEntry {
                term: Term(1),
                index: LogIndex(1),
                command: Command::noop(),
            },
            LogEntry {
                term: Term(1),
                index: LogIndex(2),
                command: ReplicatedCommand::Write(write_batch(1, 10))
                    .encode()
                    .unwrap(),
            },
        ])
        .unwrap();
        s.save_hard_state(&HardState {
            current_term: Term(1),
            voted_for: None,
            commit_index: LogIndex(2),
        })
        .unwrap();
    }
    assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 0);
    let identity = ClusterStore::new(&data_dir)
        .init(None, None, "0.4.1")
        .unwrap();
    let _cn = ClusterNode::bootstrap(
        engine.clone(),
        identity,
        ClusterConfig::single_node(),
        &raft,
    )
    .unwrap();
    assert_eq!(
        engine.find(&FindQuery::new("C")).unwrap().len(),
        1,
        "committed-but-unapplied entry recovered on open"
    );
}

#[test]
fn partial_apply_recovers_without_duplicate() {
    // The engine already applied index 2; recovery applies only index 3 and does
    // not re-apply index 2.
    let (engine, dir) = open_engine_with_schema();
    let data_dir = dir.path().join("data");
    let raft = data_dir.join("cluster");
    // Pre-apply the first write directly at commit ts 2 (base 0).
    apply_command(&engine, &ReplicatedCommand::Write(write_batch(1, 10)), 2).unwrap();
    assert_eq!(engine.commit_watermark(), 2);
    {
        let mut s = FileStorage::open(&raft).unwrap();
        s.append(&[
            LogEntry {
                term: Term(1),
                index: LogIndex(1),
                command: Command::noop(),
            },
            LogEntry {
                term: Term(1),
                index: LogIndex(2),
                command: ReplicatedCommand::Write(write_batch(1, 10))
                    .encode()
                    .unwrap(),
            },
            LogEntry {
                term: Term(1),
                index: LogIndex(3),
                command: ReplicatedCommand::Write(write_batch(2, 20))
                    .encode()
                    .unwrap(),
            },
        ])
        .unwrap();
        s.save_hard_state(&HardState {
            current_term: Term(1),
            voted_for: None,
            commit_index: LogIndex(3),
        })
        .unwrap();
    }
    let identity = ClusterStore::new(&data_dir)
        .init(None, None, "0.4.1")
        .unwrap();
    let _cn = ClusterNode::bootstrap(
        engine.clone(),
        identity,
        ClusterConfig::single_node(),
        &raft,
    )
    .unwrap();
    // Both records present exactly once (record 1 not duplicated by replay).
    let rows = engine.find(&FindQuery::new("C")).unwrap();
    assert_eq!(rows.len(), 2, "partial apply recovered without duplicate");
}

#[test]
fn apply_before_watermark_update_is_safe() {
    // Re-applying an already-applied committed index is a no-op (idempotent),
    // which is what makes a crash between apply and watermark persistence safe.
    let (engine, _dir) = open_engine_with_schema();
    apply_command(&engine, &ReplicatedCommand::Write(write_batch(1, 10)), 5).unwrap();
    assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 1);
    // The watermark advanced; replaying at or below it changes nothing.
    apply_command(&engine, &ReplicatedCommand::Write(write_batch(1, 10)), 5).unwrap();
    apply_command(&engine, &ReplicatedCommand::Write(write_batch(1, 10)), 3).unwrap();
    let rows = engine.find(&FindQuery::new("C")).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "no duplicate from re-apply at/below watermark"
    );
}

#[test]
fn applied_watermark_persists_across_reopen() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    let watermark = {
        let engine = Engine::open(&data).unwrap();
        engine.create_schema(schema()).unwrap();
        apply_command(&engine, &ReplicatedCommand::Write(write_batch(1, 10)), 4).unwrap();
        engine.commit_watermark()
    };
    assert_eq!(watermark, 4);
    let engine = Engine::open(&data).unwrap();
    assert_eq!(
        engine.commit_watermark(),
        4,
        "the applied watermark persists across reopen"
    );
}
