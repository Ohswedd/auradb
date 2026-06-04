//! Storage write/read throughput benchmarks.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{CompareOp, Filter, FindQuery};
use auradb::storage::StorageOptions;
use auradb::{Engine, EngineOptions};
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn engine() -> (tempfile::TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    // Disable per-commit fsync for throughput measurement (documented trade-off).
    let opts = EngineOptions {
        storage: StorageOptions {
            sync_on_commit: false,
        },
    };
    let engine = Engine::open_with(dir.path(), opts).unwrap();
    engine
        .create_schema(CollectionSchema::new("C").with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        }))
        .unwrap();
    (dir, engine)
}

fn bench_storage(c: &mut Criterion) {
    let (_dir, engine) = engine();
    let mut counter = 0u64;
    c.bench_function("insert", |b| {
        b.iter(|| {
            let mut f = Document::new();
            f.insert("id".into(), Value::Text(format!("k{counter}")));
            counter += 1;
            black_box(engine.insert("C", f).unwrap());
        })
    });

    let mut q = FindQuery::new("C");
    q.filter = Some(Filter::Compare {
        field: "id".into(),
        op: CompareOp::Eq,
        value: Value::Text("k0".into()),
    });
    c.bench_function("point_read", |b| {
        b.iter(|| black_box(engine.find(&q).unwrap()))
    });
}

criterion_group!(benches, bench_storage);
criterion_main!(benches);
