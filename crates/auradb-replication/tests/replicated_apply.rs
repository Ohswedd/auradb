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
    apply_command, ClusterNode, ReplicatedCommand, SchemaCommand, SnapshotManifest,
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
