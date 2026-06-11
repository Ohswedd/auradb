//! Integration coverage for analyzer-aware `auradb search eval` and
//! `search eval compare-analyzers`.
//!
//! These call the handlers directly against throwaway data directories (the same
//! pattern as `search_relevance_cli.rs`), driven by the committed analyzer
//! fixtures. They pin the behaviors the v1.5.0 analyzer slice promises:
//!
//! * `default` is byte-identical to the v1.4 baseline;
//! * `simple` equals `default` on these fixtures;
//! * `ascii_fold` recovers accented matches `simple` misses;
//! * `keyword` is exact whole-field matching (it misses partial-term queries);
//! * an unknown analyzer is a loud, structured error;
//! * results are deterministic.

use std::path::{Path, PathBuf};

use auradb_cli::{cmd_compare_analyzers, cmd_search_eval_with_analyzer};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/relevance")
        .join(name)
}
fn corpus() -> PathBuf {
    fixture("analyzer_corpus.jsonl")
}
fn queries() -> PathBuf {
    fixture("analyzer_queries.jsonl")
}
fn qrels() -> PathBuf {
    fixture("analyzer_qrels.jsonl")
}
fn fresh_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Run `search eval` with an analyzer and return the parsed JSON report.
fn eval(analyzer: Option<&str>) -> serde_json::Value {
    eval_mode("bm25", analyzer)
}

/// Run `search eval` in an explicit mode with an analyzer and return the report.
fn eval_mode(mode: &str, analyzer: Option<&str>) -> serde_json::Value {
    let dir = fresh_dir();
    let json = cmd_search_eval_with_analyzer(
        dir.path(),
        &corpus(),
        &queries(),
        &qrels(),
        mode,
        10,
        None,
        None,
        0.7,
        0.3,
        analyzer,
    )
    .expect("analyzer eval succeeds");
    serde_json::from_str(&json).unwrap()
}

/// The ranked top_docs a query returned in a report.
fn top_docs(v: &serde_json::Value, query_id: &str) -> Vec<String> {
    v["per_query"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["query_id"] == query_id)
        .unwrap()["top_docs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

fn recall(v: &serde_json::Value) -> f64 {
    v["metrics"]["recall_at_k"].as_f64().unwrap()
}

#[test]
fn search_eval_reports_analyzer() {
    let v = eval(Some("simple"));
    assert_eq!(v["analyzer"], "simple");
    // Omitting the analyzer is reported as `default`.
    let d = eval(None);
    assert_eq!(d["analyzer"], "default");
}

#[test]
fn search_eval_analyzer_default() {
    // `default` and an omitted analyzer must be identical (same JSON modulo the
    // analyzer label, which is `default` for both).
    let omitted = eval(None);
    let explicit = eval(Some("default"));
    assert_eq!(omitted, explicit);
}

#[test]
fn search_eval_analyzer_simple_matches_default_on_fixture() {
    // `simple` is documented as equal to `default` on this fixture.
    let simple = eval(Some("simple"));
    let default = eval(Some("default"));
    assert_eq!(simple["metrics"], default["metrics"]);
}

#[test]
fn search_eval_ascii_fold_recovers_accents() {
    // ascii_fold matches accented documents an unfolded analyzer misses, so its
    // recall is strictly higher on the accent-bearing fixture.
    let folded = eval(Some("ascii_fold"));
    let simple = eval(Some("simple"));
    assert!(
        recall(&folded) > recall(&simple),
        "ascii_fold recall {} should exceed simple recall {}",
        recall(&folded),
        recall(&simple)
    );
}

#[test]
fn search_eval_keyword_differs_on_fixture_if_expected() {
    // keyword is exact whole-field matching: on partial-term queries it retrieves
    // strictly fewer relevant documents than `simple`, so its recall is lower.
    let keyword = eval(Some("keyword"));
    let simple = eval(Some("simple"));
    assert!(
        recall(&keyword) < recall(&simple),
        "keyword recall {} should be below simple recall {}",
        recall(&keyword),
        recall(&simple)
    );
    // It still finds the exactly-matching document (aq-001 -> ad-001).
    let aq001 = keyword["per_query"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["query_id"] == "aq-001")
        .unwrap();
    assert!(aq001["top_docs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d == "ad-001"));
}

#[test]
fn search_eval_unknown_analyzer_errors() {
    let dir = fresh_dir();
    let err = cmd_search_eval_with_analyzer(
        dir.path(),
        &corpus(),
        &queries(),
        &qrels(),
        "bm25",
        10,
        None,
        None,
        0.7,
        0.3,
        Some("stemming"),
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown analyzer"));
}

#[test]
fn search_eval_analyzer_is_deterministic() {
    // Two runs of the same analyzer produce byte-identical reports.
    let a = eval(Some("ascii_fold"));
    let b = eval(Some("ascii_fold"));
    assert_eq!(a, b);
}

#[test]
fn compare_analyzers_reports_each_leg() {
    let dir = fresh_dir();
    let json = cmd_compare_analyzers(
        dir.path(),
        &corpus(),
        &queries(),
        &qrels(),
        "bm25",
        10,
        None,
        None,
        0.7,
        0.3,
        "default,simple,ascii_fold,keyword",
    )
    .expect("compare-analyzers succeeds");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let legs = v["analyzers"].as_array().unwrap();
    assert_eq!(legs.len(), 4);
    let names: Vec<&str> = legs
        .iter()
        .map(|l| l["analyzer"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["default", "simple", "ascii_fold", "keyword"]);
    // The comparison reproduces the per-analyzer recall relationships.
    let recall_of = |name: &str| -> f64 {
        legs.iter().find(|l| l["analyzer"] == name).unwrap()["metrics"]["recall_at_k"]
            .as_f64()
            .unwrap()
    };
    assert_eq!(recall_of("default"), recall_of("simple"));
    assert!(recall_of("ascii_fold") > recall_of("simple"));
    assert!(recall_of("keyword") < recall_of("simple"));
}

#[test]
fn search_eval_english_basic_regression() {
    // english_basic must fold `lens`/`lenses` to the SAME real term `lens` (recall
    // preserved) without truncating the bare-`s` singular to `len`. The negative
    // probe query "len" proves the regression is fixed: before the `ns` guard,
    // `lens` folded to `len`, so a `len` query wrongly retrieved the lens document.
    let eb = eval(Some("english_basic"));
    assert_eq!(eb["analyzer"], "english_basic");
    // "lens" retrieves the lens document (folds map both sides to "lens").
    assert!(
        top_docs(&eb, "aq-007").contains(&"ad-009".to_string()),
        "english_basic should retrieve the lens doc for query 'lens'"
    );
    // The truncated "len" must NOT retrieve the lens document anymore.
    assert!(
        !top_docs(&eb, "aq-008").contains(&"ad-009".to_string()),
        "regression: 'len' must not match the lens doc (lens no longer folds to len)"
    );
    // Folding never drops recall below `simple` on this fixture.
    assert!(recall(&eb) >= recall(&eval(Some("simple"))));
    // Deterministic.
    assert_eq!(eb, eval(Some("english_basic")));
}

#[test]
fn search_eval_hybrid_keyword_runs() {
    // The keyword analyzer is a valid choice in hybrid mode: the harness
    // pre-analyzes the text signal into whole-field terms and fuses it with the
    // exact vector signal, producing a normal hybrid report.
    let v = eval_mode("hybrid", Some("keyword"));
    assert_eq!(v["mode"], "hybrid");
    assert_eq!(v["analyzer"], "keyword");
    assert!(v["weights"].is_object(), "hybrid report carries weights");
    // The exact whole-field keyword query still surfaces its document in the fused
    // results (text + vector both point at ad-001).
    assert!(top_docs(&v, "aq-001").contains(&"ad-001".to_string()));
    // Deterministic.
    assert_eq!(v, eval_mode("hybrid", Some("keyword")));
}

#[test]
fn compare_analyzers_hybrid_includes_keyword() {
    // compare-analyzers must accept keyword (and english_basic) as a hybrid leg.
    let dir = fresh_dir();
    let json = cmd_compare_analyzers(
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
        "default,simple,ascii_fold,keyword,english_basic",
    )
    .expect("hybrid compare-analyzers succeeds");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["mode"], "hybrid");
    let legs = v["analyzers"].as_array().unwrap();
    let names: Vec<&str> = legs
        .iter()
        .map(|l| l["analyzer"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "default",
            "simple",
            "ascii_fold",
            "keyword",
            "english_basic"
        ]
    );
}

#[test]
fn compare_analyzers_rejects_unknown_and_duplicate() {
    let dir = fresh_dir();
    assert!(cmd_compare_analyzers(
        dir.path(),
        &corpus(),
        &queries(),
        &qrels(),
        "bm25",
        10,
        None,
        None,
        0.7,
        0.3,
        "simple,bogus",
    )
    .is_err());
    let dir2 = fresh_dir();
    assert!(cmd_compare_analyzers(
        dir2.path(),
        &corpus(),
        &queries(),
        &qrels(),
        "bm25",
        10,
        None,
        None,
        0.7,
        0.3,
        "simple,simple",
    )
    .is_err());
}
