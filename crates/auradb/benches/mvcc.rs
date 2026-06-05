//! MVCC benchmarks: latest vs snapshot point lookup, version-chain reads, write
//! conflict detection, and garbage collection. All values are measured live.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{CompareOp, Filter, FindQuery, Mutation};
use auradb::Engine;
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn schema() -> CollectionSchema {
    CollectionSchema::new("C")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("v", FieldType::Int))
}

fn doc(id: usize, v: i64) -> Document {
    let mut m = Document::new();
    m.insert("id".into(), Value::Text(format!("r{id}")));
    m.insert("v".into(), Value::Int(v));
    m
}

fn id_eq(id: usize) -> Filter {
    Filter::Compare {
        field: "id".into(),
        op: CompareOp::Eq,
        value: Value::Text(format!("r{id}")),
    }
}

fn bench_mvcc(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    for i in 0..5_000 {
        engine.insert("C", doc(i, 0)).unwrap();
    }
    engine.analyze().unwrap();

    let mut latest = FindQuery::new("C");
    latest.filter = Some(id_eq(2_500));
    c.bench_function("point_lookup_latest", |b| {
        b.iter(|| black_box(engine.find(black_box(&latest)).unwrap()))
    });

    c.bench_function("point_lookup_snapshot", |b| {
        b.iter(|| {
            let txn = engine.begin();
            let rows = engine.txn_find(&txn, black_box(&latest)).unwrap();
            engine.rollback(txn);
            black_box(rows)
        })
    });

    // Build a long version chain on one record, then read it as of an old
    // snapshot (forces a chain walk).
    let chain_txn = engine.begin();
    for v in 1..=64 {
        engine
            .apply_mutation(Mutation::Update {
                collection: "C".into(),
                filter: Some(id_eq(0)),
                set: {
                    let mut s = Document::new();
                    s.insert("v".into(), Value::Int(v));
                    s
                },
            })
            .unwrap();
    }
    let mut chain_q = FindQuery::new("C");
    chain_q.filter = Some(id_eq(0));
    c.bench_function("version_chain_snapshot_read", |b| {
        b.iter(|| black_box(engine.txn_find(&chain_txn, black_box(&chain_q)).unwrap()))
    });
    engine.rollback(chain_txn);

    c.bench_function("write_conflict_detection", |b| {
        b.iter(|| {
            // One winner commits; a concurrent writer is rejected.
            let a = engine.begin();
            let b2 = engine.begin();
            let upd = |set_v: i64| Mutation::Update {
                collection: "C".into(),
                filter: Some(id_eq(1)),
                set: {
                    let mut s = Document::new();
                    s.insert("v".into(), Value::Int(set_v));
                    s
                },
            };
            let mut ta = a;
            let mut tb = b2;
            engine.stage(&mut ta, upd(1)).unwrap();
            engine.stage(&mut tb, upd(2)).unwrap();
            engine.commit(ta).unwrap();
            let _ = black_box(engine.commit(tb)); // expected conflict
        })
    });

    c.bench_function("gc", |b| b.iter(|| black_box(engine.gc().unwrap())));
}

criterion_group!(benches, bench_mvcc);
criterion_main!(benches);
