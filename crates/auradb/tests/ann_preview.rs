//! Opt-in approximate vector search (HNSW) **preview** — v1.2.0.
//!
//! Exact vector search remains the default and the correctness baseline; the
//! preview is opt-in per query via `FindQuery::vector_ann`. These tests drive it
//! end-to-end through the engine: recall against the exact baseline, exact
//! fallback when not requested, determinism, EXPLAIN shape, and parameter /
//! dimension validation.

use std::collections::HashSet;

use auradb::core::{CollectionSchema, Document, ErrorCode, FieldDef, FieldType, Value};
use auradb::query::{AnnParams, FindQuery, VectorSearch};
use auradb::Engine;

const DIM: usize = 16;

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

fn gen_vec(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1);
    (0..DIM)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s % 2000) as f32) / 1000.0 - 1.0
        })
        .collect()
}

fn doc(id: usize, vec: Vec<f32>) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(format!("d{id}")));
    f.insert("embedding".into(), Value::Vector(vec));
    f
}

fn exact_query(vec: Vec<f32>, k: usize) -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec,
        k,
        metric: "cosine".into(),
    });
    q
}

fn ann_query(vec: Vec<f32>, k: usize, ann: AnnParams) -> FindQuery {
    let mut q = exact_query(vec, k);
    q.vector_ann = Some(ann);
    q
}

fn row_ids(rows: &[auradb::query::Row]) -> Vec<String> {
    rows.iter()
        .map(|r| match r.fields.get("id") {
            Some(Value::Text(s)) => s.clone(),
            _ => String::new(),
        })
        .collect()
}

fn seeded(dir: &std::path::Path, n: usize) -> Engine {
    let engine = Engine::open(dir).unwrap();
    engine.create_schema(schema()).unwrap();
    for i in 0..n {
        engine.insert("Doc", doc(i, gen_vec(i as u64 + 1))).unwrap();
    }
    engine
}

#[test]
fn ann_preview_recall_against_exact_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path(), 600);
    let k = 10;
    let queries = 30;
    let mut hits = 0usize;
    for q in 0..queries {
        let query = gen_vec(1_000_000 + q as u64);
        let exact: Vec<String> = row_ids(&engine.find(&exact_query(query.clone(), k)).unwrap());
        let approx: HashSet<String> = row_ids(
            &engine
                .find(&ann_query(query, k, AnnParams::default()))
                .unwrap(),
        )
        .into_iter()
        .collect();
        hits += exact.iter().filter(|id| approx.contains(*id)).count();
    }
    let recall = hits as f64 / (k * queries) as f64;
    assert!(
        recall >= 0.85,
        "ANN preview recall@{k} was {recall:.3} (< 0.85) vs the exact baseline"
    );
}

#[test]
fn ann_preview_not_used_unless_requested() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path(), 50);
    // No vector_ann -> exact path; the EXPLAIN reports approximate = false.
    let plan = engine.explain(&exact_query(gen_vec(42), 5)).unwrap();
    let v = plan.vector.expect("vector plan present");
    assert!(!v.approximate, "exact by default");
    assert!(v.ef_search.is_none());
    assert!(v.vectors_scored.is_some(), "exact reports the scan size");
}

#[test]
fn ann_preview_explain_shape() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path(), 80);
    let q = ann_query(
        gen_vec(7),
        5,
        AnnParams {
            m: Some(8),
            ef_construction: Some(64),
            ef_search: Some(40),
        },
    );
    let plan = engine.explain(&q).unwrap();
    let v = plan.vector.expect("vector plan present");
    assert!(v.approximate, "approximate preview reported");
    assert_eq!(v.ef_search, Some(40));
    assert!(
        v.vectors_scored.is_none(),
        "approximate omits the scan count"
    );
}

#[test]
fn ann_preview_deterministic_with_seeded_dataset() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path(), 300);
    let q = ann_query(gen_vec(555), 10, AnnParams::default());
    let a = row_ids(&engine.find(&q).unwrap());
    let b = row_ids(&engine.find(&q).unwrap());
    assert_eq!(a, b, "the preview is deterministic for a fixed dataset");
    assert_eq!(a.len(), 10);
}

#[test]
fn ann_preview_invalid_params_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path(), 20);
    let bad_m = ann_query(
        gen_vec(1),
        5,
        AnnParams {
            m: Some(0),
            ..Default::default()
        },
    );
    assert_eq!(
        engine.find(&bad_m).err().unwrap().code(),
        ErrorCode::InvalidRequest
    );
    let bad_ef = ann_query(
        gen_vec(1),
        5,
        AnnParams {
            ef_construction: Some(0),
            ..Default::default()
        },
    );
    assert_eq!(
        engine.find(&bad_ef).err().unwrap().code(),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn ann_preview_dimension_mismatch_rejected_and_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path(), 20);
    // A wrong-dimension query vector (the value 1234.5 must not leak into the error).
    let q = ann_query(vec![1234.5, 6789.0], 5, AnnParams::default());
    let err = engine.find(&q).err().unwrap();
    assert_eq!(err.code(), ErrorCode::InvalidRequest);
    let msg = err.to_string();
    assert!(
        msg.contains("dimension"),
        "names the dimension problem: {msg}"
    );
    assert!(
        !msg.contains("1234.5"),
        "the query payload is not echoed: {msg}"
    );
}

#[test]
fn ann_preview_rebuilds_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let _ = seeded(dir.path(), 200);
    }
    // Reopen: the graph rebuilds from the persisted exact vectors on first use.
    let engine = Engine::open(dir.path()).unwrap();
    let q = ann_query(gen_vec(321), 10, AnnParams::default());
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 10, "ANN preview works after restart");
}

#[test]
fn exact_search_unchanged_by_ann_addition() {
    // The exact path is byte-for-byte the same as before: a query with no
    // vector_ann returns the exact top-k (the fallback / baseline is always
    // available).
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path(), 200);
    let query = gen_vec(999);
    let exact = row_ids(&engine.find(&exact_query(query.clone(), 10)).unwrap());

    // Brute-force reference.
    let mut reference: Vec<(String, f32)> = (0..200)
        .map(|i| {
            let v = gen_vec(i as u64 + 1);
            let dot: f32 = query.iter().zip(&v).map(|(a, b)| a * b).sum();
            let nq: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
            let nv: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            (
                format!("d{i}"),
                if nq == 0.0 || nv == 0.0 {
                    0.0
                } else {
                    dot / (nq * nv)
                },
            )
        })
        .collect();
    reference.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
    let want: Vec<String> = reference.into_iter().take(10).map(|(id, _)| id).collect();
    assert_eq!(exact, want, "exact search remains the correctness baseline");
}
