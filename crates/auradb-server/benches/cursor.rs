//! Server-side cursor paging benchmarks.

use std::time::Duration;

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::FindQuery;
use auradb::Engine;
use auradb_server::CursorRegistry;
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn bench_cursor(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
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
    for i in 0..5_000 {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("k{i}")));
        engine.insert("C", f).unwrap();
    }
    let registry = CursorRegistry::new(Duration::from_secs(60));

    c.bench_function("open_and_drain_cursor_5k_page100", |b| {
        b.iter(|| {
            let planned = engine.plan_find(&FindQuery::new("C")).unwrap();
            let id = registry.open(FindQuery::new("C"), planned.ordered);
            let mut total = 0;
            loop {
                let page = registry.fetch(id, 100, &engine).unwrap();
                total += page.rows.len();
                if !page.more {
                    break;
                }
            }
            black_box(total);
        })
    });
}

criterion_group!(benches, bench_cursor);
criterion_main!(benches);
