//! Query planner benchmarks: planning time and indexed-vs-scan execution.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{CompareOp, Filter, FindQuery};
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
        .with_field(FieldDef {
            name: "status".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: true,
            indexed: true,
        })
        .with_field(FieldDef::new("note", FieldType::String))
}

fn doc(i: usize) -> Document {
    let mut m = Document::new();
    m.insert("id".into(), Value::Text(format!("r{i}")));
    m.insert(
        "status".into(),
        Value::Text(if i % 100 == 0 { "rare" } else { "common" }.into()),
    );
    m.insert("note".into(), Value::Text("not indexed".into()));
    m
}

fn bench_planner(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    for i in 0..5_000 {
        engine.insert("C", doc(i)).unwrap();
    }
    engine.analyze().unwrap();

    // Planning time: EXPLAIN does the plan work without materializing many rows.
    let mut indexed = FindQuery::new("C");
    indexed.filter = Some(Filter::Compare {
        field: "status".into(),
        op: CompareOp::Eq,
        value: Value::Text("rare".into()),
    });
    c.bench_function("planning_time", |b| {
        b.iter(|| black_box(engine.explain(black_box(&indexed)).unwrap()))
    });

    // Indexed query (selective secondary index) vs equivalent non-indexed scan.
    c.bench_function("indexed_query", |b| {
        b.iter(|| black_box(engine.find(black_box(&indexed)).unwrap()))
    });

    let mut scan = FindQuery::new("C");
    scan.filter = Some(Filter::Compare {
        field: "note".into(), // not indexed -> full scan
        op: CompareOp::Eq,
        value: Value::Text("not indexed".into()),
    });
    c.bench_function("scan_query", |b| {
        b.iter(|| black_box(engine.find(black_box(&scan)).unwrap()))
    });
}

criterion_group!(benches, bench_planner);
criterion_main!(benches);
