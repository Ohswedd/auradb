//! Ranked-retrieval relevance metrics: MRR@k, NDCG@k, and Recall@k.
//!
//! These are pure, deterministic functions over a ranked list of document ids
//! and a set of graded relevance judgments (qrels). They have no dependency on
//! the storage engine or the query executor, so they can be unit-tested in
//! isolation and reused by the CLI search-evaluation harness.
//!
//! Conventions:
//!
//! * A ranked list is the ordered sequence of returned document ids, best first.
//! * A relevance *grade* is a small non-negative integer (typically `0..=3`):
//!   `0` means "not relevant", larger means "more relevant".
//! * For the binary metrics (MRR, Recall) a document counts as relevant when its
//!   grade is at least [`RELEVANT_GRADE_THRESHOLD`].
//! * NDCG uses the graded judgments directly with the standard exponential gain
//!   `2^grade - 1` and a `log2(rank + 1)` position discount.
//!
//! All scores are in `[0, 1]`. The metrics are dataset-specific: they describe
//! how a ranker ordered *these* documents for *these* queries, and are not a
//! universal benchmark of search quality.

use std::collections::{HashMap, HashSet};

/// The minimum relevance grade at which a document is treated as relevant for the
/// binary metrics (MRR and Recall). Grade `0` is "not relevant"; any grade `>= 1`
/// counts as a relevant hit.
pub const RELEVANT_GRADE_THRESHOLD: u32 = 1;

/// Mean reciprocal rank truncated at `k`: the reciprocal of the 1-based position
/// of the first relevant document within the top `k`, or `0.0` if no relevant
/// document appears in the top `k`.
///
/// This is the per-query reciprocal rank; the harness averages it across queries
/// to obtain the dataset MRR@k.
pub fn mrr_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    for (i, id) in ranked.iter().take(k).enumerate() {
        if relevant.contains(id) {
            return 1.0 / (i as f64 + 1.0);
        }
    }
    0.0
}

/// Recall at `k`: the fraction of all relevant documents that appear within the
/// top `k` results. Returns `0.0` when there are no relevant documents (an
/// undefined ratio is reported as zero rather than panicking).
pub fn recall_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    let hits = ranked
        .iter()
        .take(k)
        .filter(|id| relevant.contains(*id))
        .count();
    hits as f64 / relevant.len() as f64
}

/// Discounted cumulative gain at `k` over the ranked list, using exponential gain
/// `2^grade - 1` and a `log2(rank + 1)` discount (rank is 1-based). Documents not
/// present in `grades` contribute zero gain.
pub fn dcg_at_k(ranked: &[String], grades: &HashMap<String, u32>, k: usize) -> f64 {
    let mut dcg = 0.0;
    for (i, id) in ranked.iter().take(k).enumerate() {
        let grade = grades.get(id).copied().unwrap_or(0);
        if grade > 0 {
            let gain = 2f64.powi(grade as i32) - 1.0;
            // rank is 1-based, so the discount denominator is log2(rank + 1) =
            // log2((i + 1) + 1) = log2(i + 2).
            dcg += gain / ((i as f64) + 2.0).log2();
        }
    }
    dcg
}

/// Normalized discounted cumulative gain at `k`: [`dcg_at_k`] divided by the ideal
/// DCG (the DCG of the best possible ordering of the judged grades). Returns
/// `0.0` when the ideal DCG is zero (no positive grades), so the result is always
/// in `[0, 1]`.
pub fn ndcg_at_k(ranked: &[String], grades: &HashMap<String, u32>, k: usize) -> f64 {
    let dcg = dcg_at_k(ranked, grades, k);
    let mut ideal_grades: Vec<u32> = grades.values().copied().filter(|g| *g > 0).collect();
    ideal_grades.sort_unstable_by(|a, b| b.cmp(a));
    let mut idcg = 0.0;
    for (i, grade) in ideal_grades.iter().take(k).enumerate() {
        let gain = 2f64.powi(*grade as i32) - 1.0;
        idcg += gain / ((i as f64) + 2.0).log2();
    }
    if idcg == 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

/// Build the binary relevant-document set from graded judgments, keeping only the
/// documents whose grade is at least [`RELEVANT_GRADE_THRESHOLD`].
pub fn relevant_set(grades: &HashMap<String, u32>) -> HashSet<String> {
    grades
        .iter()
        .filter(|(_, g)| **g >= RELEVANT_GRADE_THRESHOLD)
        .map(|(id, _)| id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn grades(pairs: &[(&str, u32)]) -> HashMap<String, u32> {
        pairs.iter().map(|(id, g)| (id.to_string(), *g)).collect()
    }

    #[test]
    fn mrr_rewards_earlier_first_hit() {
        let rel: HashSet<String> = ["b".to_string()].into_iter().collect();
        // First relevant at rank 2 -> 1/2.
        assert!((mrr_at_k(&ids(&["a", "b", "c"]), &rel, 10) - 0.5).abs() < 1e-12);
        // First relevant at rank 1 -> 1.0.
        assert!((mrr_at_k(&ids(&["b", "a"]), &rel, 10) - 1.0).abs() < 1e-12);
        // Not in top-k -> 0.0.
        assert_eq!(mrr_at_k(&ids(&["a", "b"]), &rel, 1), 0.0);
        // No relevant at all -> 0.0.
        assert_eq!(mrr_at_k(&ids(&["a", "c"]), &rel, 10), 0.0);
    }

    #[test]
    fn recall_counts_relevant_in_top_k() {
        let rel: HashSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        // Two of three relevant in top-2.
        assert!((recall_at_k(&ids(&["a", "b", "x"]), &rel, 2) - 2.0 / 3.0).abs() < 1e-12);
        // All three in top-3.
        assert!((recall_at_k(&ids(&["a", "b", "c"]), &rel, 3) - 1.0).abs() < 1e-12);
        // Empty relevant set -> 0.0, no panic.
        let empty: HashSet<String> = HashSet::new();
        assert_eq!(recall_at_k(&ids(&["a"]), &empty, 3), 0.0);
    }

    #[test]
    fn ndcg_is_one_for_ideal_order_and_in_unit_interval() {
        let g = grades(&[("a", 3), ("b", 2), ("c", 1)]);
        // Ideal ordering -> 1.0.
        assert!((ndcg_at_k(&ids(&["a", "b", "c"]), &g, 3) - 1.0).abs() < 1e-12);
        // A worse ordering scores strictly less than the ideal but stays in [0,1].
        let worse = ndcg_at_k(&ids(&["c", "b", "a"]), &g, 3);
        assert!(worse < 1.0, "reordered NDCG {worse} should be < 1");
        assert!(
            (0.0..=1.0).contains(&worse),
            "NDCG {worse} must be in [0,1]"
        );
        // No judged grades -> 0.0.
        assert_eq!(ndcg_at_k(&ids(&["a"]), &HashMap::new(), 3), 0.0);
    }

    #[test]
    fn relevant_set_applies_threshold() {
        let g = grades(&[("a", 0), ("b", 1), ("c", 3)]);
        let rel = relevant_set(&g);
        assert!(!rel.contains("a"));
        assert!(rel.contains("b"));
        assert!(rel.contains("c"));
        assert_eq!(rel.len(), 2);
    }
}
