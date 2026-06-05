//! Single-node cluster overhead benchmarks.
//!
//! These measure the cost of routing writes and reads through the durable
//! single-node Raft log versus the direct (non-cluster) engine path. The numbers
//! are hardware-dependent and meant only for same-machine regression tracking;
//! they are not a universal performance claim. A single-node cluster orders every
//! write through the Raft log, so a write is expected to cost more than the direct
//! path — this bench quantifies that overhead so a regression is visible.

use std::hint::black_box;

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{FindQuery, Mutation};
use auradb::Engine;
use auradb_cluster::{ClusterConfig, ClusterStore};
use auradb_replication::ClusterNode;
use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::TempDir;

fn schema() -> CollectionSchema {
    CollectionSchema::new("Bench")
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

fn record(id: i64) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Int(id));
    f.insert("v".into(), Value::Int(id));
    f
}

/// A plain engine with no replication (the direct path).
fn direct_engine() -> (Engine, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path().join("data")).unwrap();
    engine.create_schema(schema()).unwrap();
    (engine, dir)
}

/// An engine whose writes are routed through a durable single-node Raft log.
fn cluster_engine() -> (Engine, ClusterNode, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let engine = Engine::open(&data).unwrap();
    engine.create_schema(schema()).unwrap();
    let identity = ClusterStore::new(&data).init(None, None, "0.4.1").unwrap();
    let node = ClusterNode::bootstrap(
        engine.clone(),
        identity,
        ClusterConfig::single_node(),
        data.join("cluster"),
    )
    .unwrap();
    engine.attach_replicated_log(node.write_log());
    (engine, node, dir)
}

fn bench_direct_insert(c: &mut Criterion) {
    c.bench_function("direct_insert", |b| {
        let (engine, _dir) = direct_engine();
        let mut i = 0i64;
        b.iter(|| {
            i += 1;
            engine
                .apply_mutation(Mutation::Upsert {
                    collection: "Bench".into(),
                    fields: record(i),
                })
                .unwrap();
            black_box(());
        });
    });
}

fn bench_single_node_cluster_insert(c: &mut Criterion) {
    c.bench_function("single_node_cluster_insert", |b| {
        let (engine, _node, _dir) = cluster_engine();
        let mut i = 0i64;
        b.iter(|| {
            i += 1;
            engine
                .apply_mutation(Mutation::Upsert {
                    collection: "Bench".into(),
                    fields: record(i),
                })
                .unwrap();
            black_box(());
        });
    });
}

fn bench_point_read(c: &mut Criterion) {
    // Point read of the latest version is unaffected by cluster mode (reads do
    // not go through Raft); this measures both for completeness.
    let (direct, _d) = direct_engine();
    for i in 1..=1000 {
        direct
            .apply_mutation(Mutation::Upsert {
                collection: "Bench".into(),
                fields: record(i),
            })
            .unwrap();
    }
    let (cluster, _node, _c2) = cluster_engine();
    for i in 1..=1000 {
        cluster
            .apply_mutation(Mutation::Upsert {
                collection: "Bench".into(),
                fields: record(i),
            })
            .unwrap();
    }
    let mut q = FindQuery::new("Bench");
    q.filter = Some(auradb::query::Filter::Compare {
        field: "id".into(),
        op: auradb::query::CompareOp::Eq,
        value: Value::Int(500),
    });

    c.bench_function("direct_point_read", |b| {
        b.iter(|| black_box(direct.find(&q).unwrap()));
    });
    c.bench_function("single_node_cluster_point_read", |b| {
        b.iter(|| black_box(cluster.find(&q).unwrap()));
    });
}

fn bench_status(c: &mut Criterion) {
    let (_engine, node, _dir) = cluster_engine();
    c.bench_function("single_node_cluster_status", |b| {
        b.iter(|| black_box(node.status()));
    });
}

criterion_group!(
    benches,
    bench_direct_insert,
    bench_single_node_cluster_insert,
    bench_point_read,
    bench_status
);
criterion_main!(benches);
