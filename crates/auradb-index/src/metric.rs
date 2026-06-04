//! Vector distance metrics and exact nearest-neighbour search.

use auradb_core::{Error, Result};
use serde_json::Value as Json;

/// A vector similarity metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    /// Cosine similarity (higher is more similar).
    Cosine,
    /// Euclidean (L2) distance (closer is more similar).
    Euclidean,
    /// Dot product (higher is more similar).
    DotProduct,
}

impl Metric {
    /// Parse a metric from its lowercase name.
    pub fn parse(s: &str) -> Result<Metric> {
        match s.to_ascii_lowercase().as_str() {
            "cosine" => Ok(Metric::Cosine),
            "euclidean" | "l2" => Ok(Metric::Euclidean),
            "dot" | "dot_product" | "dotproduct" => Ok(Metric::DotProduct),
            other => Err(Error::InvalidRequest(format!("unknown metric: {other}"))),
        }
    }

    /// The metric's canonical name.
    pub fn name(self) -> &'static str {
        match self {
            Metric::Cosine => "cosine",
            Metric::Euclidean => "euclidean",
            Metric::DotProduct => "dot_product",
        }
    }

    /// Compute a similarity score where **higher is always more similar**.
    ///
    /// - Cosine: cosine similarity in `[-1, 1]`.
    /// - Dot product: the raw dot product.
    /// - Euclidean: the negative L2 distance.
    pub fn similarity(self, a: &[f32], b: &[f32]) -> f32 {
        match self {
            Metric::Cosine => cosine_similarity(a, b),
            Metric::DotProduct => dot(a, b),
            Metric::Euclidean => -euclidean_distance(a, b),
        }
    }

    /// The human-facing distance for a pair (L2 for euclidean, `1 - sim` for
    /// cosine, `-dot` for dot product) where **lower is more similar**.
    pub fn distance(self, a: &[f32], b: &[f32]) -> f32 {
        match self {
            Metric::Cosine => 1.0 - cosine_similarity(a, b),
            Metric::DotProduct => -dot(a, b),
            Metric::Euclidean => euclidean_distance(a, b),
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let d = x - y;
            d * d
        })
        .sum::<f32>()
        .sqrt()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let na = dot(a, a).sqrt();
    let nb = dot(b, b).sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot(a, b) / (na * nb)
    }
}

/// Serialize a metric into a JSON string value (for EXPLAIN output).
pub fn metric_json(metric: Metric) -> Json {
    Json::String(metric.name().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((Metric::Cosine.similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn euclidean_closer_scores_higher() {
        let q = vec![0.0, 0.0];
        let near = vec![1.0, 0.0];
        let far = vec![5.0, 0.0];
        assert!(Metric::Euclidean.similarity(&q, &near) > Metric::Euclidean.similarity(&q, &far));
    }

    #[test]
    fn dot_product_orders_by_magnitude_alignment() {
        let q = vec![1.0, 1.0];
        let aligned = vec![2.0, 2.0];
        let opposed = vec![-2.0, -2.0];
        assert!(
            Metric::DotProduct.similarity(&q, &aligned)
                > Metric::DotProduct.similarity(&q, &opposed)
        );
    }

    #[test]
    fn parse_roundtrip() {
        for m in [Metric::Cosine, Metric::Euclidean, Metric::DotProduct] {
            assert_eq!(Metric::parse(m.name()).unwrap(), m);
        }
        assert!(Metric::parse("nope").is_err());
    }
}
