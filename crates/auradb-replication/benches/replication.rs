//! Benchmarks for the replication path: applying a committed command, the
//! direct vs. local-Raft write path, and snapshot create / restore.

use std::hint::black_box;

use auradb::core::{CollectionId, CollectionSchema, Document, FieldDef, FieldType, Record, Value};
use auradb::query::Mutation;
use auradb::Engine;
use auradb_cluster::{ClusterConfig, ClusterStore};
use auradb_replication::{apply_command, ClusterNode, ReplicatedCommand, SnapshotManifest};
use auradb_storage::{Batch, LogOp};
use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::tempdir;

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

fn record(id: i64) -> Record {
    let mut f = Document::new();
    f.insert("id".into(), Value::Int(id));
    f.insert("v".into(), Value::Int(id * 10));
    Record::new(
        auradb::core::RecordId::from_u128(id as u128),
        CollectionId::new("C"),
        f,
    )
}

fn write_batch(id: i64) -> Batch {
    Batch {
        txn_id: auradb::core::TxnId(id as u64),
        ops: vec![LogOp::Put {
            commit_ts: 0,
            record: record(id),
        }],
    }
}

fn bench_apply_committed(c: &mut Criterion) {
    c.bench_function("replication_apply_committed_command", |b| {
        let dir = tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        engine.create_schema(schema()).unwrap();
        let mut idx = 0u64;
        b.iter(|| {
            idx += 1;
            let cmd = ReplicatedCommand::Write(write_batch(idx as i64));
            apply_command(black_box(&engine), &cmd, idx).unwrap();
        });
    });
}

fn bench_write_path_direct_vs_raft(c: &mut Criterion) {
    let mut group = c.benchmark_group("write_path");
    group.bench_function("direct", |b| {
        let dir = tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        engine.create_schema(schema()).unwrap();
        let mut id = 0i64;
        b.iter(|| {
            id += 1;
            let mut f = Document::new();
            f.insert("id".into(), Value::Int(id));
            f.insert("v".into(), Value::Int(id));
            black_box(
                engine
                    .apply_mutation(Mutation::Upsert {
                        collection: "C".into(),
                        fields: f,
                    })
                    .unwrap(),
            );
        });
    });
    group.bench_function("local_raft", |b| {
        let dir = tempdir().unwrap();
        let engine = Engine::open(dir.path().join("data")).unwrap();
        engine.create_schema(schema()).unwrap();
        let identity = ClusterStore::new(dir.path().join("data"))
            .init(None, None, "0.4.0")
            .unwrap();
        let node = ClusterNode::bootstrap(
            engine.clone(),
            identity,
            ClusterConfig::single_node(),
            dir.path().join("data").join("cluster"),
        )
        .unwrap();
        engine.attach_replicated_log(node.write_log());
        let mut id = 0i64;
        b.iter(|| {
            id += 1;
            let mut f = Document::new();
            f.insert("id".into(), Value::Int(id));
            f.insert("v".into(), Value::Int(id));
            black_box(
                engine
                    .apply_mutation(Mutation::Upsert {
                        collection: "C".into(),
                        fields: f,
                    })
                    .unwrap(),
            );
        });
    });
    group.finish();
}

fn bench_snapshot(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    for id in 1..=1000 {
        let mut f = Document::new();
        f.insert("id".into(), Value::Int(id));
        f.insert("v".into(), Value::Int(id));
        engine
            .apply_mutation(Mutation::Insert {
                collection: "C".into(),
                fields: f,
            })
            .unwrap();
    }
    c.bench_function("snapshot_create_1k", |b| {
        b.iter(|| {
            black_box(SnapshotManifest::create(&engine, 1000, 1, "0.4.0").unwrap());
        });
    });
    let snap = SnapshotManifest::create(&engine, 1000, 1, "0.4.0").unwrap();
    c.bench_function("snapshot_restore_1k", |b| {
        let mut n = 0u64;
        b.iter(|| {
            n += 1;
            let target = dir.path().join(format!("restore-{n}"));
            black_box(snap.restore(&target).unwrap());
        });
    });
}

criterion_group!(
    benches,
    bench_apply_committed,
    bench_write_path_direct_vs_raft,
    bench_snapshot
);
criterion_main!(benches);
