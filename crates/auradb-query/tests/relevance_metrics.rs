//! Integration coverage for the public relevance-metric functions. These mirror
//! the in-module unit tests but exercise the metrics through the crate's public
//! surface, which is what the CLI search-evaluation harness depends on.

use std::collections::{HashMap, HashSet};

use auradb_query::relevance::{mrr_at_k, ndcg_at_k, recall_at_k, relevant_set};

fn ids(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn grades(pairs: &[(&str, u32)]) -> HashMap<String, u32> {
    pairs.iter().map(|(id, g)| (id.to_string(), *g)).collect()
}

#[test]
fn relevance_metrics_mrr() {
    let relevant: HashSet<String> = ["doc-002".to_string()].into_iter().collect();
    // First relevant document at rank 2 -> reciprocal rank 0.5.
    let rr = mrr_at_k(&ids(&["doc-001", "doc-002", "doc-003"]), &relevant, 10);
    assert!((rr - 0.5).abs() < 1e-12, "expected 0.5, got {rr}");
    // Relevant document outside the cutoff -> 0.0.
    assert_eq!(mrr_at_k(&ids(&["doc-001", "doc-002"]), &relevant, 1), 0.0);
}

#[test]
fn relevance_metrics_ndcg() {
    let g = grades(&[("doc-001", 3), ("doc-002", 2), ("doc-003", 1)]);
    // Ideal ordering scores 1.0; a degraded ordering scores strictly less and
    // stays within the unit interval.
    let ideal = ndcg_at_k(&ids(&["doc-001", "doc-002", "doc-003"]), &g, 3);
    assert!(
        (ideal - 1.0).abs() < 1e-12,
        "ideal NDCG should be 1.0, got {ideal}"
    );
    let degraded = ndcg_at_k(&ids(&["doc-003", "doc-001", "doc-002"]), &g, 3);
    assert!(
        degraded < ideal,
        "degraded {degraded} must be < ideal {ideal}"
    );
    assert!(
        (0.0..=1.0).contains(&degraded),
        "NDCG {degraded} out of [0,1]"
    );
}

#[test]
fn relevance_metrics_recall() {
    let g = grades(&[("doc-001", 3), ("doc-002", 2), ("doc-003", 0)]);
    let relevant = relevant_set(&g);
    // doc-003 has grade 0 and is not relevant; two relevant documents exist.
    assert_eq!(relevant.len(), 2);
    // One of two relevant documents retrieved in the top-1 -> 0.5.
    let r = recall_at_k(&ids(&["doc-001", "doc-004"]), &relevant, 1);
    assert!((r - 0.5).abs() < 1e-12, "expected 0.5, got {r}");
    // Both retrieved -> 1.0.
    let full = recall_at_k(&ids(&["doc-001", "doc-002"]), &relevant, 10);
    assert!((full - 1.0).abs() < 1e-12, "expected 1.0, got {full}");
}

#[test]
fn relevance_metrics_are_deterministic() {
    let g = grades(&[("doc-001", 3), ("doc-002", 2), ("doc-003", 1)]);
    let relevant = relevant_set(&g);
    let ranked = ids(&["doc-002", "doc-001", "doc-005", "doc-003"]);
    let first = (
        mrr_at_k(&ranked, &relevant, 10),
        ndcg_at_k(&ranked, &g, 10),
        recall_at_k(&ranked, &relevant, 10),
    );
    for _ in 0..50 {
        let again = (
            mrr_at_k(&ranked, &relevant, 10),
            ndcg_at_k(&ranked, &g, 10),
            recall_at_k(&ranked, &relevant, 10),
        );
        assert_eq!(first, again, "metrics must be deterministic");
    }
}
