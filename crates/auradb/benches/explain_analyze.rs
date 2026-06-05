//! EXPLAIN ANALYZE overhead benchmark: the cost of running a query through
//! `explain_analyze` (execute + metrics) versus a plain `find`.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{CompareOp, Filter, FindQuery};
use auradb::Engine;
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn bench_explain_analyze(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine
        .create_schema(
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
                }),
        )
        .unwrap();
    for i in 0..5_000 {
        let mut m = Document::new();
        m.insert("id".into(), Value::Text(format!("r{i}")));
        m.insert(
            "status".into(),
            Value::Text(if i % 50 == 0 { "rare" } else { "common" }.into()),
        );
        engine.insert("C", m).unwrap();
    }
    engine.analyze().unwrap();

    let mut q = FindQuery::new("C");
    q.filter = Some(Filter::Compare {
        field: "status".into(),
        op: CompareOp::Eq,
        value: Value::Text("rare".into()),
    });

    c.bench_function("find", |b| {
        b.iter(|| black_box(engine.find(black_box(&q)).unwrap()))
    });
    c.bench_function("explain_analyze", |b| {
        b.iter(|| black_box(engine.explain_analyze(black_box(&q)).unwrap()))
    });
}

criterion_group!(benches, bench_explain_analyze);
criterion_main!(benches);
