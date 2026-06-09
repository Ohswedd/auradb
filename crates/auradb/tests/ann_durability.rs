//! ANN/HNSW preview durability and exact-fallback (v1.3.0).
//!
//! The approximate (HNSW) graph is never persisted — it rebuilds in memory from
//! the exact vectors on first use — but its lifecycle **metadata** is now durable
//! (additive index-snapshot field, no storage-format change). These tests cover:
//! metadata surviving a checkpoint + restart, the exact-fallback policy when the
//! preview is unavailable (below the minimum-dataset threshold), the
//! `ann_fallback = error` path, and EXPLAIN `vector_mode` reporting. Exact search
//! remains the correctness baseline throughout.

use std::collections::HashSet;

use auradb::core::{CollectionSchema, Document, ErrorCode, FieldDef, FieldType, Value};
use auradb::query::{AnnFallback, AnnParams, FindQuery, VectorSearch, ANN_PREVIEW_MIN_VECTORS};
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

fn seeded(dir: &std::path::Path, n: usize) -> Engine {
    let engine = Engine::open(dir).unwrap();
    engine.create_schema(schema()).unwrap();
    for i in 0..n {
        engine.insert("Doc", doc(i, gen_vec(i as u64 + 1))).unwrap();
    }
    engine
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

fn ann_query(vec: Vec<f32>, k: usize, fallback: AnnFallback) -> FindQuery {
    let mut q = exact_query(vec, k);
    q.vector_ann = Some(AnnParams {
        fallback,
        ..Default::default()
    });
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

#[test]
fn hnsw_metadata_persists_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = seeded(dir.path(), 50);
        // Checkpoint writes the index snapshot, including the durable ANN preview
        // metadata for the vector field.
        engine.checkpoint().unwrap();
    }
    // Reopen: the snapshot loads, so the metadata is present (generation marker)
    // and the field is reported as preview-eligible.
    let engine = Engine::open(dir.path()).unwrap();
    let report = engine.search_index_report();
    let coll = report
        .iter()
        .find(|c| c.collection == "Doc")
        .expect("Doc reported");
    let vf = coll
        .vector_fields
        .iter()
        .find(|v| v.field == "embedding")
        .expect("embedding vector field reported");
    assert_eq!(vf.vectors, 50);
    assert_eq!(vf.dim, DIM);
    assert!(vf.ann_preview_eligible, "50 >= threshold");
    assert_eq!(vf.ann_preview_status, "ready_on_use");
    assert!(
        vf.ann_generation.is_some(),
        "durable generation marker loaded from the snapshot"
    );
}

#[test]
fn hnsw_metadata_round_trips_below_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let small = ANN_PREVIEW_MIN_VECTORS - 1;
    {
        let engine = seeded(dir.path(), small);
        engine.checkpoint().unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    let report = engine.search_index_report();
    let vf = report
        .iter()
        .find(|c| c.collection == "Doc")
        .and_then(|c| c.vector_fields.iter().find(|v| v.field == "embedding"))
        .expect("vector field reported");
    assert_eq!(vf.vectors, small);
    assert!(!vf.ann_preview_eligible, "below threshold");
    assert_eq!(vf.ann_preview_status, "exact_only_below_threshold");
    // Metadata still persisted (a generation marker exists) even though the
    // preview is not eligible at this size.
    assert!(vf.ann_generation.is_some());
}

#[test]
fn hnsw_exact_fallback_matches_exact_results() {
    // Below the threshold, an approximate request with the default `exact`
    // fallback returns the exact top-k — byte-for-byte the exact baseline.
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path(), ANN_PREVIEW_MIN_VECTORS - 4);
    let query = gen_vec(123);
    let exact = row_ids(&engine.find(&exact_query(query.clone(), 5)).unwrap());
    let fallback = row_ids(
        &engine
            .find(&ann_query(query, 5, AnnFallback::Exact))
            .unwrap(),
    );
    assert_eq!(
        exact, fallback,
        "exact fallback returns the exact baseline results"
    );
}

#[test]
fn hnsw_query_require_ann_errors_when_unavailable() {
    // With `ann_fallback = error`, a below-threshold approximate request returns a
    // structured InvalidRequest instead of silently using exact search.
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path(), ANN_PREVIEW_MIN_VECTORS - 4);
    let err = engine
        .find(&ann_query(gen_vec(7), 5, AnnFallback::Error))
        .expect_err("require-ann errors when the preview is unavailable");
    assert_eq!(err.code(), ErrorCode::InvalidRequest);
    let msg = err.to_string();
    assert!(msg.contains("unavailable"), "explains why: {msg}");
}

#[test]
fn hnsw_explain_reports_vector_mode() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path(), 64);

    // Exact (no ann).
    let v = engine
        .explain(&exact_query(gen_vec(1), 5))
        .unwrap()
        .vector
        .unwrap();
    assert_eq!(v.vector_mode.as_deref(), Some("exact"));
    assert!(!v.exact_fallback);
    assert!(!v.approximate);

    // Eligible field -> ann_preview.
    let v = engine
        .explain(&ann_query(gen_vec(1), 5, AnnFallback::Exact))
        .unwrap()
        .vector
        .unwrap();
    assert_eq!(v.vector_mode.as_deref(), Some("ann_preview"));
    assert!(!v.exact_fallback);
    assert!(v.approximate);

    // Below-threshold field -> exact_fallback.
    let small_dir = tempfile::tempdir().unwrap();
    let small = seeded(small_dir.path(), ANN_PREVIEW_MIN_VECTORS - 2);
    let v = small
        .explain(&ann_query(gen_vec(1), 5, AnnFallback::Exact))
        .unwrap()
        .vector
        .unwrap();
    assert_eq!(v.vector_mode.as_deref(), Some("exact_fallback"));
    assert!(v.exact_fallback);
    assert!(!v.approximate, "fallback runs exact, not approximate");
}

#[test]
fn hnsw_recall_probe_against_exact_baseline() {
    // A deterministic recall probe: the preview's top-k overlaps the exact top-k
    // well above a conservative floor on a fixed seeded dataset. This is a
    // dataset-specific guard, not a universal recall guarantee.
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path(), 500);
    let k = 10;
    let queries = 20;
    let mut hits = 0usize;
    for q in 0..queries {
        let query = gen_vec(2_000_000 + q as u64);
        let exact: Vec<String> = row_ids(&engine.find(&exact_query(query.clone(), k)).unwrap());
        let approx: HashSet<String> = row_ids(
            &engine
                .find(&ann_query(query, k, AnnFallback::Exact))
                .unwrap(),
        )
        .into_iter()
        .collect();
        hits += exact.iter().filter(|id| approx.contains(*id)).count();
    }
    let recall = hits as f64 / (k * queries) as f64;
    assert!(recall >= 0.85, "preview recall@{k} {recall:.3} < 0.85");
}
