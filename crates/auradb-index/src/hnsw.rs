//! Approximate vector search — Hierarchical Navigable Small World (HNSW) preview.
//!
//! This is an **opt-in preview** approximate nearest-neighbour index. Exact
//! vector search (see [`crate::ExactVectorIndex`]) remains the default and the
//! correctness baseline; this graph trades a small, tunable amount of recall for
//! sub-linear query cost. It is built **in memory from the authoritative exact
//! vectors** (never persisted), so storage format v2 is unchanged — the graph is
//! a derived query accelerator, rebuilt on demand.
//!
//! The implementation follows Malkov & Yashunin (2016): a layered proximity
//! graph with a logarithmic layer assignment, greedy descent through the upper
//! layers, and an `ef`-width beam search at the base layer. Navigation minimises
//! `-similarity` so the ordering objective is identical to exact search (which
//! ranks by similarity descending) — only the candidate set is approximate.
//!
//! Determinism: layer assignment uses a seeded SplitMix64 PRNG, so a graph built
//! from the same vectors in the same order is reproducible (the recall tests rely
//! on this).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use auradb_core::RecordId;

use crate::metric::Metric;
use crate::Neighbor;

/// HNSW construction parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HnswParams {
    /// Target out-degree per node on the upper layers (`M`). The base layer
    /// allows up to `2*M` to keep it well connected.
    pub m: usize,
    /// Beam width used while inserting (`efConstruction`). Larger builds a better
    /// graph at higher build cost.
    pub ef_construction: usize,
}

impl Default for HnswParams {
    fn default() -> Self {
        HnswParams {
            m: 16,
            ef_construction: 200,
        }
    }
}

impl HnswParams {
    /// Validate the parameters, returning a message on the offending value.
    pub fn validate(&self) -> Result<(), String> {
        if self.m == 0 || self.m > 512 {
            return Err(format!("hnsw `m` must be in 1..=512 (got {})", self.m));
        }
        if self.ef_construction == 0 || self.ef_construction > 4096 {
            return Err(format!(
                "hnsw `ef_construction` must be in 1..=4096 (got {})",
                self.ef_construction
            ));
        }
        Ok(())
    }

    /// The level-assignment normalization factor `mL = 1 / ln(M)`.
    fn level_mult(&self) -> f64 {
        1.0 / (self.m.max(2) as f64).ln()
    }
}

/// A total order over `f32` for the search heaps (NaN sorts last, deterministic).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ord32(f32);
impl Eq for Ord32 {}
impl PartialOrd for Ord32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Ord32 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// One graph node: its per-layer neighbour adjacency (internal indices).
#[derive(Debug, Default)]
struct Node {
    /// `neighbors[layer]` is the adjacency list at that layer.
    neighbors: Vec<Vec<usize>>,
}

/// An in-memory HNSW graph over a set of `dim`-dimensional vectors under a fixed
/// metric. Built once (from the exact vectors) and queried many times.
#[derive(Debug)]
pub struct Hnsw {
    metric: Metric,
    params: HnswParams,
    ids: Vec<RecordId>,
    vectors: Vec<Vec<f32>>,
    nodes: Vec<Node>,
    entry: Option<usize>,
    max_layer: usize,
    rng: u64,
}

impl Hnsw {
    /// Build a graph from `(id, vector)` pairs under `metric` and `params`.
    /// `seed` makes layer assignment deterministic.
    pub fn build(
        entries: impl IntoIterator<Item = (RecordId, Vec<f32>)>,
        metric: Metric,
        params: HnswParams,
        seed: u64,
    ) -> Self {
        let mut g = Hnsw {
            metric,
            params,
            ids: Vec::new(),
            vectors: Vec::new(),
            nodes: Vec::new(),
            entry: None,
            max_layer: 0,
            rng: seed | 1,
        };
        for (id, v) in entries {
            g.insert(id, v);
        }
        g
    }

    /// The number of indexed vectors.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether the graph is empty.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The metric this graph was built with.
    pub fn metric(&self) -> Metric {
        self.metric
    }

    /// SplitMix64 — a fast, well-distributed seeded PRNG (deterministic).
    fn next_u64(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform `f64` in `(0, 1)`.
    fn next_unit(&mut self) -> f64 {
        // 53-bit mantissa; add a tiny epsilon to avoid exactly 0.
        let bits = self.next_u64() >> 11;
        (bits as f64 / (1u64 << 53) as f64).max(f64::MIN_POSITIVE)
    }

    /// Draw a layer for a new node: `floor(-ln(u) * mL)`.
    fn assign_layer(&mut self) -> usize {
        let u = self.next_unit();
        (-u.ln() * self.params.level_mult()).floor() as usize
    }

    /// `-similarity` between the query and stored vector `idx` (lower = closer,
    /// matching the exact ranking objective of "highest similarity first").
    fn dist_to(&self, query: &[f32], idx: usize) -> f32 {
        -self.metric.similarity(query, &self.vectors[idx])
    }

    /// The maximum out-degree at a layer (`2*M` at the base, `M` above).
    fn max_degree(&self, layer: usize) -> usize {
        if layer == 0 {
            self.params.m * 2
        } else {
            self.params.m
        }
    }

    /// Greedy beam search at `layer`, returning up to `ef` closest candidates to
    /// `query` (as `(neg_dist, idx)` so the caller can drain closest-first). The
    /// search starts from `entry_points`.
    fn search_layer(
        &self,
        query: &[f32],
        entry_points: &[usize],
        ef: usize,
        layer: usize,
    ) -> Vec<(f32, usize)> {
        let mut visited: std::collections::HashSet<usize> = std::collections::HashSet::new();
        // Candidate frontier: min-heap by distance (closest popped first).
        let mut frontier: BinaryHeap<Reverse<(Ord32, usize)>> = BinaryHeap::new();
        // Result set: max-heap by distance (farthest on top, evicted when > ef).
        let mut results: BinaryHeap<(Ord32, usize)> = BinaryHeap::new();

        for &ep in entry_points {
            let d = self.dist_to(query, ep);
            visited.insert(ep);
            frontier.push(Reverse((Ord32(d), ep)));
            results.push((Ord32(d), ep));
        }
        while results.len() > ef {
            results.pop();
        }

        while let Some(Reverse((Ord32(cand_d), cand))) = frontier.pop() {
            // The current farthest kept result bounds the search.
            let farthest = results
                .peek()
                .map(|(Ord32(d), _)| *d)
                .unwrap_or(f32::INFINITY);
            if cand_d > farthest && results.len() >= ef {
                break;
            }
            if let Some(node) = self.nodes.get(cand) {
                if let Some(adj) = node.neighbors.get(layer) {
                    for &nb in adj {
                        if !visited.insert(nb) {
                            continue;
                        }
                        let d = self.dist_to(query, nb);
                        let farthest = results
                            .peek()
                            .map(|(Ord32(d), _)| *d)
                            .unwrap_or(f32::INFINITY);
                        if results.len() < ef || d < farthest {
                            frontier.push(Reverse((Ord32(d), nb)));
                            results.push((Ord32(d), nb));
                            while results.len() > ef {
                                results.pop();
                            }
                        }
                    }
                }
            }
        }

        results.into_iter().map(|(Ord32(d), i)| (d, i)).collect()
    }

    /// Select up to `m` closest of `candidates` (simple heuristic). The
    /// candidates already carry their distance to the inserted vector.
    fn select_neighbors(&self, mut candidates: Vec<(f32, usize)>, m: usize) -> Vec<usize> {
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(self.ids[a.1].cmp(&self.ids[b.1])));
        candidates.into_iter().take(m).map(|(_, i)| i).collect()
    }

    /// Insert `(id, vector)` into the graph.
    pub fn insert(&mut self, id: RecordId, vector: Vec<f32>) {
        let idx = self.ids.len();
        let layer = self.assign_layer();
        self.ids.push(id);
        self.vectors.push(vector);
        self.nodes.push(Node {
            neighbors: vec![Vec::new(); layer + 1],
        });

        let Some(entry) = self.entry else {
            self.entry = Some(idx);
            self.max_layer = layer;
            return;
        };

        let query = self.vectors[idx].clone();
        let mut ep = vec![entry];

        // Descend from the top of the graph down to just above the new node's
        // top layer, greedily (ef = 1).
        let top = self.max_layer;
        let mut lc = top;
        while lc > layer {
            let found = self.search_layer(&query, &ep, 1, lc);
            if let Some(&(_, best)) = found.iter().min_by(|a, b| a.0.total_cmp(&b.0)) {
                ep = vec![best];
            }
            lc -= 1;
        }

        // For each layer from min(layer, top) down to 0, connect to M neighbours.
        let start = layer.min(top);
        for lc in (0..=start).rev() {
            let candidates = self.search_layer(&query, &ep, self.params.ef_construction, lc);
            let m = self.params.m;
            let selected = self.select_neighbors(candidates.clone(), m);
            // Connect bidirectionally.
            for &nb in &selected {
                self.nodes[idx].neighbors[lc].push(nb);
                self.nodes[nb].neighbors[lc].push(idx);
                // Prune the neighbour's adjacency if it grew past the budget.
                let budget = self.max_degree(lc);
                if self.nodes[nb].neighbors[lc].len() > budget {
                    self.prune(nb, lc, budget);
                }
            }
            ep = candidates.into_iter().map(|(_, i)| i).collect();
            if ep.is_empty() {
                ep = vec![entry];
            }
        }

        if layer > self.max_layer {
            self.max_layer = layer;
            self.entry = Some(idx);
        }
    }

    /// Re-prune node `n`'s neighbours at `layer` to the `budget` closest.
    fn prune(&mut self, n: usize, layer: usize, budget: usize) {
        let base = self.vectors[n].clone();
        let mut adj: Vec<(f32, usize)> = self.nodes[n].neighbors[layer]
            .iter()
            .map(|&nb| (-self.metric.similarity(&base, &self.vectors[nb]), nb))
            .collect();
        adj.sort_by(|a, b| a.0.total_cmp(&b.0).then(self.ids[a.1].cmp(&self.ids[b.1])));
        adj.truncate(budget);
        self.nodes[n].neighbors[layer] = adj.into_iter().map(|(_, i)| i).collect();
    }

    /// Approximate `k` nearest neighbours to `query` with beam width `ef_search`.
    /// Results are re-ranked by exact similarity and returned highest-first, the
    /// same ordering exact search uses (so a perfect-recall result is identical).
    pub fn nearest(&self, query: &[f32], k: usize, ef_search: usize) -> Vec<Neighbor> {
        if self.is_empty() || k == 0 {
            return Vec::new();
        }
        let Some(entry) = self.entry else {
            return Vec::new();
        };
        let mut ep = vec![entry];
        let mut lc = self.max_layer;
        while lc > 0 {
            let found = self.search_layer(query, &ep, 1, lc);
            if let Some(&(_, best)) = found.iter().min_by(|a, b| a.0.total_cmp(&b.0)) {
                ep = vec![best];
            }
            lc -= 1;
        }
        let ef = ef_search.max(k);
        let candidates = self.search_layer(query, &ep, ef, 0);

        // Re-rank by exact similarity (score desc, id asc) — identical to exact.
        let mut scored: Vec<Neighbor> = candidates
            .into_iter()
            .map(|(_, idx)| Neighbor {
                id: self.ids[idx],
                score: self.metric.similarity(query, &self.vectors[idx]),
                distance: self.metric.distance(query, &self.vectors[idx]),
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.id.cmp(&b.id))
        });
        scored.truncate(k);
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen_vec(seed: u64, dim: usize) -> Vec<f32> {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        (0..dim)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                ((s % 2000) as f32) / 1000.0 - 1.0
            })
            .collect()
    }

    /// Brute-force exact top-k (the recall reference).
    fn exact_top_k(
        vectors: &[(RecordId, Vec<f32>)],
        query: &[f32],
        k: usize,
        metric: Metric,
    ) -> Vec<RecordId> {
        let mut scored: Vec<(f32, RecordId)> = vectors
            .iter()
            .map(|(id, v)| (metric.similarity(query, v), *id))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        scored.into_iter().take(k).map(|(_, id)| id).collect()
    }

    #[test]
    fn hnsw_recall_against_exact_baseline() {
        let dim = 16;
        let n = 1000;
        let vectors: Vec<(RecordId, Vec<f32>)> = (0..n)
            .map(|i| {
                (
                    RecordId::from_u128(i as u128 + 1),
                    gen_vec(i as u64 + 1, dim),
                )
            })
            .collect();

        for metric in [Metric::Cosine, Metric::Euclidean] {
            let g = Hnsw::build(vectors.clone(), metric, HnswParams::default(), 42);
            assert_eq!(g.len(), n);

            let k = 10;
            let queries = 50;
            let mut hits = 0usize;
            for q in 0..queries {
                let query = gen_vec(1_000_000 + q as u64, dim);
                let approx: std::collections::HashSet<RecordId> = g
                    .nearest(&query, k, 100)
                    .into_iter()
                    .map(|nb| nb.id)
                    .collect();
                let exact = exact_top_k(&vectors, &query, k, metric);
                hits += exact.iter().filter(|id| approx.contains(id)).count();
            }
            let recall = hits as f64 / (k * queries) as f64;
            assert!(
                recall >= 0.90,
                "metric {metric:?}: HNSW recall@{k} was {recall:.3} (< 0.90)"
            );
        }
    }

    #[test]
    fn hnsw_is_deterministic_for_a_seed() {
        let dim = 8;
        let vectors: Vec<(RecordId, Vec<f32>)> = (0..200)
            .map(|i| {
                (
                    RecordId::from_u128(i as u128 + 1),
                    gen_vec(i as u64 + 1, dim),
                )
            })
            .collect();
        let query = gen_vec(99, dim);
        let a = Hnsw::build(vectors.clone(), Metric::Cosine, HnswParams::default(), 7);
        let b = Hnsw::build(vectors.clone(), Metric::Cosine, HnswParams::default(), 7);
        let ra: Vec<RecordId> = a
            .nearest(&query, 10, 64)
            .into_iter()
            .map(|n| n.id)
            .collect();
        let rb: Vec<RecordId> = b
            .nearest(&query, 10, 64)
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(ra, rb, "same seed + data must give identical results");
    }

    #[test]
    fn hnsw_perfect_recall_matches_exact_ordering_small() {
        // With ef >= n the base-layer search visits everything, so the result is
        // exactly the exact top-k in the exact order.
        let dim = 6;
        let vectors: Vec<(RecordId, Vec<f32>)> = (0..40)
            .map(|i| {
                (
                    RecordId::from_u128(i as u128 + 1),
                    gen_vec(i as u64 + 3, dim),
                )
            })
            .collect();
        let g = Hnsw::build(vectors.clone(), Metric::Cosine, HnswParams::default(), 5);
        let query = gen_vec(123, dim);
        let approx: Vec<RecordId> = g
            .nearest(&query, 10, 200)
            .into_iter()
            .map(|n| n.id)
            .collect();
        let exact = exact_top_k(&vectors, &query, 10, Metric::Cosine);
        assert_eq!(approx, exact, "ef >= n yields exact top-k in exact order");
    }

    #[test]
    fn params_validation() {
        assert!(HnswParams {
            m: 0,
            ef_construction: 100
        }
        .validate()
        .is_err());
        assert!(HnswParams {
            m: 16,
            ef_construction: 0
        }
        .validate()
        .is_err());
        assert!(HnswParams::default().validate().is_ok());
    }
}
