//! Query (filter) execution benchmarks.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{CompareOp, Filter, FindQuery};
use auradb::Engine;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_query(c: &mut Criterion) {
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
                })
                .with_field(FieldDef::new("views", FieldType::Int)),
        )
        .unwrap();
    for i in 0..5_000 {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("k{i}")));
        f.insert(
            "status".into(),
            Value::Text(if i % 2 == 0 { "published" } else { "draft" }.into()),
        );
        f.insert("views".into(), Value::Int(i));
        engine.insert("C", f).unwrap();
    }

    let mut indexed = FindQuery::new("C");
    indexed.filter = Some(Filter::Compare {
        field: "status".into(),
        op: CompareOp::Eq,
        value: Value::Text("published".into()),
    });
    c.bench_function("indexed_filter_5k", |b| {
        b.iter(|| black_box(engine.find(&indexed).unwrap()))
    });

    let mut scan = FindQuery::new("C");
    scan.filter = Some(Filter::Compare {
        field: "views".into(),
        op: CompareOp::Gte,
        value: Value::Int(2500),
    });
    c.bench_function("full_scan_filter_5k", |b| {
        b.iter(|| black_box(engine.find(&scan).unwrap()))
    });
}

criterion_group!(benches, bench_query);
criterion_main!(benches);
