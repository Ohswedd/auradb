//! `auradb vector eval` recall/latency harness (v1.3.0).
//!
//! The harness measures approximate (HNSW preview) recall@k and latency against
//! the exact baseline over a deterministic query set, emitting JSON. It returns
//! only real measured data and never echoes the query vectors.

use std::io::Write;

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::Engine;
use auradb_cli::cmd_vector_eval;

const DIM: usize = 16;

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

fn seed_dir(n: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine
        .create_schema(
            CollectionSchema::new("Doc")
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
    for i in 0..n {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("d{i}")));
        f.insert("embedding".into(), Value::Vector(gen_vec(i as u64 + 1)));
        engine.insert("Doc", f).unwrap();
    }
    engine.checkpoint().unwrap();
    dir
}

fn write_queries(lines: &[Vec<f32>]) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    for v in lines {
        writeln!(f, "{}", serde_json::to_string(v).unwrap()).unwrap();
    }
    f.flush().unwrap();
    f
}

#[test]
fn vector_eval_reports_recall() {
    let dir = seed_dir(400);
    let queries: Vec<Vec<f32>> = (0..10).map(|q| gen_vec(3_000_000 + q)).collect();
    let qfile = write_queries(&queries);

    let json = cmd_vector_eval(
        dir.path(),
        "Doc",
        "embedding",
        qfile.path(),
        10,
        "cosine",
        64,
    )
    .expect("eval succeeds");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["collection"], "Doc");
    assert_eq!(v["field"], "embedding");
    assert_eq!(v["queries"], 10);
    assert_eq!(v["k"], 10);
    assert_eq!(v["ef_search"], 64);
    let mean = v["mean_recall_at_k"].as_f64().unwrap();
    assert!((0.0..=1.0).contains(&mean), "recall in [0,1]: {mean}");
    assert!(mean >= 0.8, "preview recall {mean} should clear 0.8 here");
    assert!(v["exact_latency_ms_p50"].as_f64().unwrap() >= 0.0);
    assert!(v["ann_latency_ms_p50"].as_f64().unwrap() >= 0.0);
}

#[test]
fn vector_eval_rejects_dimension_mismatch() {
    let dir = seed_dir(50);
    // A wrong-dimension query vector.
    let qfile = write_queries(&[vec![1.0, 2.0, 3.0]]);
    let err = cmd_vector_eval(
        dir.path(),
        "Doc",
        "embedding",
        qfile.path(),
        5,
        "cosine",
        64,
    )
    .expect_err("dimension mismatch is rejected");
    let msg = err.to_string().to_lowercase();
    assert!(msg.contains("dimension") || msg.contains("dim"), "{msg}");
}

#[test]
fn vector_eval_json_shape_stable_and_no_secret_leak() {
    let dir = seed_dir(100);
    // Use a distinctive sentinel value in the query so we can prove it does not
    // leak into the report.
    let mut q = gen_vec(42);
    q[0] = 987654.0;
    let qfile = write_queries(&[q]);
    let json = cmd_vector_eval(
        dir.path(),
        "Doc",
        "embedding",
        qfile.path(),
        5,
        "cosine",
        32,
    )
    .expect("eval succeeds");
    assert!(
        !json.contains("987654"),
        "query vector payload must not appear in the report"
    );
    // Stable shape: required keys present.
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    for key in [
        "collection",
        "field",
        "metric",
        "queries",
        "k",
        "ef_search",
        "mean_recall_at_k",
        "min_recall_at_k",
        "exact_latency_ms_p50",
        "ann_latency_ms_p50",
    ] {
        assert!(v.get(key).is_some(), "missing key {key}");
    }
}

#[test]
fn vector_eval_requires_queries() {
    let dir = seed_dir(20);
    let qfile = write_queries(&[]);
    let err = cmd_vector_eval(
        dir.path(),
        "Doc",
        "embedding",
        qfile.path(),
        5,
        "cosine",
        64,
    )
    .expect_err("empty query set is rejected");
    assert!(err.to_string().contains("no query vectors"));
}
