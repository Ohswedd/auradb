//! Exact vector search remains the correctness baseline (no production ANN).
//! These tests strengthen that baseline: a larger-dataset correctness regression
//! against a brute-force reference, EXPLAIN ANALYZE shape, restart round-trip,
//! and dimension-error redaction (the error must not echo the query vector).

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{FindQuery, Strategy, VectorSearch};
use auradb::Engine;

const DIM: usize = 8;

fn schema() -> CollectionSchema {
    CollectionSchema::new("Doc")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: DIM }))
}

fn doc(id: usize, vec: Vec<f32>) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(format!("d{id}")));
    f.insert("embedding".into(), Value::Vector(vec));
    f
}

/// A deterministic pseudo-random vector from a seed (no RNG dependency).
fn gen_vec(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1);
    (0..DIM)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s % 1000) as f32) / 1000.0
        })
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

fn vector_query(vec: Vec<f32>, k: usize) -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec,
        k,
        metric: "cosine".into(),
    });
    q
}

fn row_id(row: &auradb::query::Row) -> String {
    match row.fields.get("id") {
        Some(Value::Text(s)) => s.clone(),
        _ => String::new(),
    }
}

#[test]
fn exact_vector_large_dataset_regression() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();

    let n = 2000usize;
    let mut vectors = Vec::with_capacity(n);
    for i in 0..n {
        let v = gen_vec(i as u64 + 1);
        vectors.push((format!("d{i}"), v.clone()));
        engine.insert("Doc", doc(i, v)).unwrap();
    }

    let query = gen_vec(123_456);
    let k = 10;
    let rows = engine.find(&vector_query(query.clone(), k)).unwrap();
    assert_eq!(rows.len(), k);

    // Brute-force reference: top-k by cosine, tie-broken by id (matching the
    // engine's deterministic ordering).
    let mut reference: Vec<(String, f32)> = vectors
        .iter()
        .map(|(id, v)| (id.clone(), cosine(&query, v)))
        .collect();
    reference.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
    let expected: Vec<String> = reference.into_iter().take(k).map(|(id, _)| id).collect();
    let got: Vec<String> = rows.iter().map(row_id).collect();
    assert_eq!(got, expected, "exact search must match brute-force top-k");
}

#[test]
fn exact_vector_explain_analyze_shape() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    for i in 0..20 {
        engine.insert("Doc", doc(i, gen_vec(i as u64 + 1))).unwrap();
    }
    let plan = engine
        .explain_analyze(&vector_query(gen_vec(7), 5))
        .unwrap();
    assert_eq!(plan.strategy, Strategy::VectorExactScan);
    let vp = plan.vector.expect("vector summary present");
    assert_eq!(vp.field, "embedding");
    assert_eq!(vp.k, 5);
    assert_eq!(vp.metric, "cosine");
    let analysis = plan.analysis.expect("analysis present");
    assert_eq!(analysis.returned_rows, 5);
}

#[test]
fn exact_vector_after_restart_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let query = gen_vec(42);
    let expected_top = {
        let engine = Engine::open(dir.path()).unwrap();
        engine.create_schema(schema()).unwrap();
        for i in 0..200 {
            engine.insert("Doc", doc(i, gen_vec(i as u64 + 1))).unwrap();
        }
        engine.checkpoint().unwrap();
        row_id(&engine.find(&vector_query(query.clone(), 1)).unwrap()[0])
    };
    // Reopen: the vector index loads from the snapshot and returns the same top hit.
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(engine.index_load_report().rebuilt, 0);
    let after = row_id(&engine.find(&vector_query(query, 1)).unwrap()[0]);
    assert_eq!(after, expected_top);
}

#[test]
fn exact_vector_bruteforce_equivalence() {
    // Exact search must equal a brute-force ranking on a varied dataset for
    // several independent queries and k values.
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    let n = 500usize;
    let mut vectors = Vec::with_capacity(n);
    for i in 0..n {
        let v = gen_vec(i as u64 * 7 + 3);
        vectors.push((format!("d{i}"), v.clone()));
        engine.insert("Doc", doc(i, v)).unwrap();
    }
    for (seed, k) in [(11u64, 1usize), (222, 5), (3333, 25), (44444, 100)] {
        let query = gen_vec(seed);
        let got: Vec<String> = engine
            .find(&vector_query(query.clone(), k))
            .unwrap()
            .iter()
            .map(row_id)
            .collect();
        let mut reference: Vec<(String, f32)> = vectors
            .iter()
            .map(|(id, v)| (id.clone(), cosine(&query, v)))
            .collect();
        reference.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
        let expected: Vec<String> = reference.into_iter().take(k).map(|(id, _)| id).collect();
        assert_eq!(got, expected, "exact != brute force for seed {seed} k {k}");
    }
}

#[test]
fn exact_vector_explain_analyze_reports_scored_count() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    for i in 0..37 {
        engine.insert("Doc", doc(i, gen_vec(i as u64 + 1))).unwrap();
    }
    let plan = engine
        .explain_analyze(&vector_query(gen_vec(9), 5))
        .unwrap();
    let vp = plan.vector.expect("vector summary");
    // The exact scan compares every indexed vector for the field.
    assert_eq!(vp.vectors_scored, Some(37));
    assert_eq!(plan.analysis.unwrap().returned_rows, 5);
}

#[test]
fn exact_vector_dimension_error_redaction() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    engine.insert("Doc", doc(1, gen_vec(1))).unwrap();
    // Query with a wrong, distinctive payload.
    let bad = vec![123.456_f32, 789.012, 345.678];
    let err = engine
        .find(&vector_query(bad, 5))
        .expect_err("dimension mismatch should error");
    let msg = err.to_string();
    // Reports the dimensions but does not echo the raw query vector values.
    assert!(msg.contains('3') && msg.contains('8'), "msg: {msg}");
    assert!(
        !msg.contains("123.456"),
        "error must not echo the payload: {msg}"
    );
}
