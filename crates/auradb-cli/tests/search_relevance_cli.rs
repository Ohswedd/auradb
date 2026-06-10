//! Integration coverage for `auradb search eval` (the `cmd_search_eval` handler).
//!
//! Tests call the handler directly against a throwaway data directory — the same
//! pattern as the other CLI command tests — so they need no running server and no
//! external downloads. The committed relevance fixtures drive the regression
//! cases; crafted temp datasets drive the parsing/validation cases.

use std::path::{Path, PathBuf};

use auradb_cli::cmd_search_eval;

/// Resolve a committed relevance fixture by file name.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/relevance")
        .join(name)
}

fn corpus() -> PathBuf {
    fixture("small_corpus.jsonl")
}
fn queries() -> PathBuf {
    fixture("small_queries.jsonl")
}
fn qrels() -> PathBuf {
    fixture("small_qrels.jsonl")
}

/// A fresh data directory (the harness ingests the corpus and requires an empty
/// directory).
fn fresh_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Write `lines` to a temp JSONL file and return its path (kept alive by the dir).
fn write_jsonl(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, lines.join("\n")).unwrap();
    path
}

fn run_bm25(data_dir: &Path) -> serde_json::Value {
    let json = cmd_search_eval(
        data_dir,
        &corpus(),
        &queries(),
        &qrels(),
        "bm25",
        10,
        None,
        None,
        0.7,
        0.3,
    )
    .expect("bm25 eval succeeds");
    serde_json::from_str(&json).unwrap()
}

#[test]
fn search_eval_json_shape() {
    let dir = fresh_dir();
    let v = run_bm25(dir.path());
    assert_eq!(v["dataset"], "small");
    assert_eq!(v["mode"], "bm25");
    assert_eq!(v["queries"], 5);
    assert_eq!(v["documents"], 12);
    assert_eq!(v["k"], 10);
    // Aggregate metrics block with the three required metrics.
    for key in ["mrr_at_k", "ndcg_at_k", "recall_at_k"] {
        assert!(v["metrics"][key].is_number(), "missing metrics.{key}");
        let val = v["metrics"][key].as_f64().unwrap();
        assert!(
            (0.0..=1.0).contains(&val),
            "metric {key}={val} out of [0,1]"
        );
    }
    // Per-query block, one entry per query, each with metrics and top_docs.
    let per_query = v["per_query"].as_array().unwrap();
    assert_eq!(per_query.len(), 5);
    let first = &per_query[0];
    assert!(first["query_id"].is_string());
    assert!(first["top_docs"].is_array());
    assert!(v["warnings"].is_array());
}

#[test]
fn search_eval_fixture_regression() {
    let dir = fresh_dir();
    let v = run_bm25(dir.path());
    // The fixture is hand-built so every query's first relevant document ranks
    // first (MRR=1.0) and all relevant documents are recalled within k (Recall=1.0).
    let mrr = v["metrics"]["mrr_at_k"].as_f64().unwrap();
    let recall = v["metrics"]["recall_at_k"].as_f64().unwrap();
    let ndcg = v["metrics"]["ndcg_at_k"].as_f64().unwrap();
    assert!((mrr - 1.0).abs() < 1e-9, "MRR regression: {mrr}");
    assert!((recall - 1.0).abs() < 1e-9, "Recall regression: {recall}");
    // NDCG is high but not perfect (graded ties differ from the engine ordering).
    assert!(
        ndcg > 0.9 && ndcg <= 1.0,
        "NDCG out of expected band: {ndcg}"
    );
    assert!(
        v["warnings"].as_array().unwrap().is_empty(),
        "unexpected warnings"
    );
}

#[test]
fn search_eval_is_deterministic() {
    let a = run_bm25(fresh_dir().path());
    let b = run_bm25(fresh_dir().path());
    assert_eq!(a, b, "search eval output must be deterministic");
}

#[test]
fn bm25_eval_reports_defaults() {
    let dir = fresh_dir();
    let v = run_bm25(dir.path());
    assert_eq!(v["preset"], "default");
    assert!((v["bm25"]["k1"].as_f64().unwrap() - 1.2).abs() < 1e-6);
    assert!((v["bm25"]["b"].as_f64().unwrap() - 0.75).abs() < 1e-6);
}

#[test]
fn bm25_eval_custom_params_reported() {
    let dir = fresh_dir();
    let json = cmd_search_eval(
        dir.path(),
        &corpus(),
        &queries(),
        &qrels(),
        "bm25",
        10,
        Some(0.9),
        Some(0.4),
        0.7,
        0.3,
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["preset"], "custom");
    assert!((v["bm25"]["k1"].as_f64().unwrap() - 0.9).abs() < 1e-6);
    assert!((v["bm25"]["b"].as_f64().unwrap() - 0.4).abs() < 1e-6);
}

#[test]
fn hybrid_eval_runs() {
    let dir = fresh_dir();
    let json = cmd_search_eval(
        dir.path(),
        &corpus(),
        &queries(),
        &qrels(),
        "hybrid",
        10,
        None,
        None,
        0.7,
        0.3,
    )
    .expect("hybrid eval succeeds");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["mode"], "hybrid");
    assert!(v["metrics"]["recall_at_k"].as_f64().unwrap() >= 0.9);
}

#[test]
fn hybrid_eval_reports_weights_if_supported() {
    let dir = fresh_dir();
    let json = cmd_search_eval(
        dir.path(),
        &corpus(),
        &queries(),
        &qrels(),
        "hybrid",
        10,
        None,
        None,
        0.6,
        0.4,
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!((v["weights"]["text"].as_f64().unwrap() - 0.6).abs() < 1e-6);
    assert!((v["weights"]["vector"].as_f64().unwrap() - 0.4).abs() < 1e-6);
    // Both BM25 params and fusion weights are present for hybrid.
    assert!(v["bm25"].is_object());
}

#[test]
fn hybrid_eval_rejects_bad_weights() {
    let dir = fresh_dir();
    // Both weights zero is rejected.
    assert!(cmd_search_eval(
        dir.path(),
        &corpus(),
        &queries(),
        &qrels(),
        "hybrid",
        10,
        None,
        None,
        0.0,
        0.0,
    )
    .is_err());
    let dir2 = fresh_dir();
    // A negative weight is rejected.
    assert!(cmd_search_eval(
        dir2.path(),
        &corpus(),
        &queries(),
        &qrels(),
        "hybrid",
        10,
        None,
        None,
        -0.5,
        1.0,
    )
    .is_err());
}

#[test]
fn vector_exact_eval_runs() {
    let dir = fresh_dir();
    let json = cmd_search_eval(
        dir.path(),
        &corpus(),
        &queries(),
        &qrels(),
        "vector_exact",
        10,
        None,
        None,
        0.7,
        0.3,
    )
    .expect("vector_exact eval succeeds");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["mode"], "vector_exact");
    // Text-only fields are absent for a pure vector run.
    assert!(v["bm25"].is_null());
    assert!(v["weights"].is_null());
}

#[test]
fn relevance_dataset_parse_corpus() {
    let dir = fresh_dir();
    let tmp = tempfile::tempdir().unwrap();
    let c = write_jsonl(
        tmp.path(),
        "c.jsonl",
        &[
            r#"{"id":"d1","title":"alpha","body":"alpha beta"}"#,
            r#"{"id":"d2","title":"gamma","body":"gamma delta"}"#,
        ],
    );
    let q = write_jsonl(tmp.path(), "q.jsonl", &[r#"{"id":"q1","text":"alpha"}"#]);
    let r = write_jsonl(
        tmp.path(),
        "r.jsonl",
        &[r#"{"query_id":"q1","doc_id":"d1","relevance":3}"#],
    );
    let json = cmd_search_eval(dir.path(), &c, &q, &r, "bm25", 10, None, None, 0.7, 0.3).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["documents"], 2);
    assert_eq!(v["queries"], 1);
}

#[test]
fn relevance_dataset_rejects_missing_ids() {
    let dir = fresh_dir();
    let tmp = tempfile::tempdir().unwrap();
    // Corpus row missing the required `id` field.
    let c = write_jsonl(tmp.path(), "c.jsonl", &[r#"{"title":"no id here"}"#]);
    let q = write_jsonl(tmp.path(), "q.jsonl", &[r#"{"id":"q1","text":"x"}"#]);
    let r = write_jsonl(
        tmp.path(),
        "r.jsonl",
        &[r#"{"query_id":"q1","doc_id":"d1","relevance":1}"#],
    );
    assert!(cmd_search_eval(dir.path(), &c, &q, &r, "bm25", 10, None, None, 0.7, 0.3).is_err());
}

#[test]
fn relevance_dataset_rejects_bad_relevance() {
    let dir = fresh_dir();
    let tmp = tempfile::tempdir().unwrap();
    let c = write_jsonl(tmp.path(), "c.jsonl", &[r#"{"id":"d1","body":"x"}"#]);
    let q = write_jsonl(tmp.path(), "q.jsonl", &[r#"{"id":"q1","text":"x"}"#]);
    // Negative relevance grade is rejected.
    let r = write_jsonl(
        tmp.path(),
        "r.jsonl",
        &[r#"{"query_id":"q1","doc_id":"d1","relevance":-1}"#],
    );
    assert!(cmd_search_eval(dir.path(), &c, &q, &r, "bm25", 10, None, None, 0.7, 0.3).is_err());
}

#[test]
fn search_eval_malformed_dataset_nonzero() {
    let dir = fresh_dir();
    let tmp = tempfile::tempdir().unwrap();
    // A corpus line that is not valid JSON.
    let c = write_jsonl(tmp.path(), "c.jsonl", &["this is not json"]);
    let q = write_jsonl(tmp.path(), "q.jsonl", &[r#"{"id":"q1","text":"x"}"#]);
    let r = write_jsonl(
        tmp.path(),
        "r.jsonl",
        &[r#"{"query_id":"q1","doc_id":"d1","relevance":1}"#],
    );
    assert!(cmd_search_eval(dir.path(), &c, &q, &r, "bm25", 10, None, None, 0.7, 0.3).is_err());
}

#[test]
fn search_eval_unknown_mode_rejected() {
    let dir = fresh_dir();
    assert!(cmd_search_eval(
        dir.path(),
        &corpus(),
        &queries(),
        &qrels(),
        "nonsense",
        10,
        None,
        None,
        0.7,
        0.3,
    )
    .is_err());
}

#[test]
fn search_eval_warns_on_unknown_qrel_ids() {
    let dir = fresh_dir();
    let tmp = tempfile::tempdir().unwrap();
    let c = write_jsonl(tmp.path(), "c.jsonl", &[r#"{"id":"d1","body":"alpha"}"#]);
    let q = write_jsonl(tmp.path(), "q.jsonl", &[r#"{"id":"q1","text":"alpha"}"#]);
    let r = write_jsonl(
        tmp.path(),
        "r.jsonl",
        &[
            r#"{"query_id":"q1","doc_id":"d1","relevance":2}"#,
            r#"{"query_id":"q1","doc_id":"ghost","relevance":1}"#,
        ],
    );
    let json = cmd_search_eval(dir.path(), &c, &q, &r, "bm25", 10, None, None, 0.7, 0.3).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("ghost")),
        "expected a warning about the unknown doc id"
    );
}
