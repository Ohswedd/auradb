//! Exact vector search benchmarks.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{FindQuery, VectorSearch};
use auradb::Engine;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

const DIM: usize = 64;

fn pseudo_vector(seed: usize) -> Vec<f32> {
    // Deterministic pseudo-random vector (no RNG dependency).
    (0..DIM)
        .map(|i| (((seed.wrapping_mul(2654435761) + i * 40503) % 1000) as f32) / 1000.0)
        .collect()
}

fn bench_vector(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine
        .create_schema(
            CollectionSchema::new("V")
                .with_field(FieldDef {
                    name: "id".into(),
                    field_type: FieldType::Uuid,
                    primary_key: true,
                    unique: true,
                    nullable: false,
                    indexed: false,
                })
                .with_field(FieldDef::new("embedding", FieldType::Vector { dim: DIM })),
        )
        .unwrap();
    let n = 10_000;
    for i in 0..n {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("v{i}")));
        f.insert("embedding".into(), Value::Vector(pseudo_vector(i)));
        engine.insert("V", f).unwrap();
    }

    let mut q = FindQuery::new("V");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: pseudo_vector(42),
        k: 10,
        metric: "cosine".into(),
    });
    c.bench_function("vector_top10_over_10k_dim64", |b| {
        b.iter(|| black_box(engine.find(&q).unwrap()))
    });
}

criterion_group!(benches, bench_vector);
criterion_main!(benches);
