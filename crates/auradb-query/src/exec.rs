//! Query execution: candidate selection, filtering, ordering, projection,
//! relationship hydration, and EXPLAIN planning.

use std::cmp::Ordering;
use std::time::{Duration, Instant};

use auradb_core::{Cardinality, CollectionSchema, Error, Record, RecordId, Result, Value};
use auradb_index::{CollectionIndexes, HnswParams, Metric};

/// Default HNSW `efSearch` for the approximate vector preview when a query does
/// not specify one (clamped up to at least `k` at the call site).
const DEFAULT_ANN_EF_SEARCH: usize = 64;
/// Default HNSW graph degree `M`.
const DEFAULT_ANN_M: usize = 16;
/// Default HNSW `efConstruction`.
const DEFAULT_ANN_EF_CONSTRUCTION: usize = 200;

/// A cooperative execution deadline. Long-running read paths poll [`check`] at
/// bounded intervals; when the elapsed wall-clock time exceeds the budget the
/// poll returns a structured [`Error::QueryTimeout`] so the in-flight query is
/// abandoned cleanly without tearing down the session.
///
/// A deadline of `0` milliseconds (or [`Deadline::none`]) disables the check, so
/// the default behaviour for callers that pass no timeout is unchanged.
///
/// [`check`]: Deadline::check
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    start: Instant,
    limit: Option<Duration>,
    limit_ms: u64,
}

/// Poll the deadline at least this often while iterating a candidate set, so the
/// timeout-check overhead stays negligible on the hot path while still bounding
/// how long an over-budget scan can run past its deadline.
const DEADLINE_POLL_INTERVAL: usize = 1024;

impl Deadline {
    /// A deadline that never fires.
    pub fn none() -> Self {
        Deadline {
            start: Instant::now(),
            limit: None,
            limit_ms: 0,
        }
    }

    /// A deadline `limit_ms` milliseconds from now. `0` disables the deadline.
    pub fn after_ms(limit_ms: u64) -> Self {
        Deadline {
            start: Instant::now(),
            limit: (limit_ms > 0).then(|| Duration::from_millis(limit_ms)),
            limit_ms,
        }
    }

    /// Whether this deadline can ever fire.
    pub fn is_enabled(&self) -> bool {
        self.limit.is_some()
    }

    /// Return a [`Error::QueryTimeout`] if the budget has already been exceeded.
    pub fn check(&self) -> Result<()> {
        if let Some(limit) = self.limit {
            let elapsed = self.start.elapsed();
            if elapsed > limit {
                return Err(Error::query_timeout(elapsed.as_millis(), self.limit_ms));
            }
        }
        Ok(())
    }

    /// Poll the deadline only every [`DEADLINE_POLL_INTERVAL`] iterations,
    /// keeping the per-iteration cost off the hot path. `i` is the 0-based
    /// iteration index.
    #[inline]
    pub(crate) fn check_at(&self, i: usize) -> Result<()> {
        if self.limit.is_some() && i % DEADLINE_POLL_INTERVAL == 0 {
            self.check()?;
        }
        Ok(())
    }
}

use crate::eval;
use crate::ir::{
    CountQuery, ExistsQuery, FindQuery, FusionMode, HybridSearch, OrderKey, Row, TextRank,
    TextSearch, BM25_DEFAULT_B, BM25_DEFAULT_K1,
};
use crate::plan::{Access, PlanNode};
use crate::planner;
use crate::stats::CollectionStats;

/// Read-only access to the engine's data and indexes, implemented by `auradb`.
pub trait DataSource {
    /// The schema for a collection, if registered.
    fn schema(&self, collection: &str) -> Option<&CollectionSchema>;
    /// The indexes for a collection, if registered.
    fn indexes(&self, collection: &str) -> Option<&CollectionIndexes>;
    /// All live records in a collection.
    fn scan<'a>(&'a self, collection: &str) -> Box<dyn Iterator<Item = &'a Record> + 'a>;
    /// A single record by collection and id.
    fn get(&self, collection: &str, id: RecordId) -> Option<&Record>;
    /// Resolve a relationship link: find the record in `target` whose primary
    /// key equals `key`. Engines derive the internal id from the key.
    fn resolve_link(&self, target: &str, key: &Value) -> Option<&Record>;
    /// Persisted planner statistics for a collection, if available. Statistics
    /// are advisory; the default returns `None` and the planner falls back to
    /// live row counts and default selectivity.
    fn stats(&self, _collection: &str) -> Option<&CollectionStats> {
        None
    }
    /// The persisted planner-statistics format version, when statistics are
    /// available. Reported by `EXPLAIN ANALYZE` for diagnostics. The default is
    /// `None` (no persisted statistics).
    fn stats_version(&self) -> Option<u32> {
        None
    }
}

/// The selection strategy chosen by the planner.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Exact vector scan over a vector index, then post-filtering.
    VectorExactScan,
    /// Unranked full-text candidate selection seeded by an inverted index
    /// (the `contains_text` boolean predicate).
    FullTextScan,
    /// Ranked full-text (BM25) search seeded by an inverted index.
    FullTextBm25,
    /// Hybrid text-plus-vector ranked retrieval with score fusion.
    Hybrid,
    /// Equality lookup seeded by a secondary/unique/primary index.
    IndexLookup,
    /// Full collection scan with filtering.
    FullScan,
}

/// An EXPLAIN plan describing how a query will run.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExplainPlan {
    /// The queried collection.
    pub collection: String,
    /// The selection strategy.
    pub strategy: Strategy,
    /// The index used to seed selection, if any.
    pub used_index: Option<String>,
    /// Estimated number of candidate records examined.
    pub estimated_candidates: usize,
    /// Whether a filter is applied.
    pub filter_present: bool,
    /// Vector clause summary, if present.
    pub vector: Option<VectorPlan>,
    /// Ranked full-text clause summary, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_search: Option<TextSearchPlan>,
    /// Hybrid clause summary, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hybrid: Option<HybridPlan>,
    /// Ordering keys.
    pub order_by: Vec<OrderKey>,
    /// Relationships hydrated.
    pub includes: Vec<String>,
    /// Non-fatal planner warnings.
    pub warnings: Vec<String>,
    /// Planner row estimate for the chosen access path.
    #[serde(default)]
    pub estimated_rows: usize,
    /// Planner cost estimate for the chosen plan (lower is better).
    #[serde(default)]
    pub estimated_cost: f64,
    /// The full plan tree chosen by the planner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_tree: Option<PlanNode>,
    /// Execution metrics, present only for `EXPLAIN ANALYZE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<ExplainAnalysis>,
}

/// Measured execution metrics attached by `EXPLAIN ANALYZE`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExplainAnalysis {
    /// Candidate rows produced by the access path before the residual filter.
    pub scanned_rows: usize,
    /// Rows that passed the filter (matched).
    pub matched_rows: usize,
    /// Rows returned after offset/limit.
    pub returned_rows: usize,
    /// Total wall-clock execution time in microseconds.
    pub execution_micros: u128,
    /// Planning time in microseconds.
    pub planning_micros: u128,
    /// The snapshot read timestamp, when executed within a transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_ts: Option<u64>,
    /// The planner row estimate for the chosen access path (mirrors
    /// `ExplainPlan::estimated_rows`, placed here so a single ANALYZE object
    /// carries both the estimate and the measured `matched_rows`).
    #[serde(default)]
    pub estimated_rows: usize,
    /// The persisted planner-statistics format version used for planning, if
    /// statistics were available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_stats_version: Option<u32>,
    /// A short, human-readable reason the planner selected its access path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_index_reason: Option<String>,
    /// Whether the planner statistics looked stale or absent when planning (the
    /// plan is still correct; the cost choice may be suboptimal).
    #[serde(default)]
    pub stale_stats: bool,
}

/// Vector clause summary in an EXPLAIN plan.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VectorPlan {
    /// The searched field.
    pub field: String,
    /// Requested neighbour count.
    pub k: usize,
    /// The metric.
    pub metric: String,
    /// Number of indexed vectors compared by the exact (brute-force) scan — the
    /// size of the vector index for this field. Reported so an operator can see
    /// the exact-search cost; exact search remains the correctness baseline (there
    /// is no approximate/ANN index in v1.1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vectors_scored: Option<usize>,
    /// Whether the opt-in approximate (HNSW) **preview** index served this query.
    /// `false` (the default) means exact search — the correctness baseline.
    #[serde(default)]
    pub approximate: bool,
    /// The HNSW `efSearch` beam width used, when `approximate` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ef_search: Option<usize>,
}

/// Ranked full-text clause summary in an EXPLAIN plan.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TextSearchPlan {
    /// The searched full-text field.
    pub field: String,
    /// The ranking mode (`bm25` or `term_frequency`).
    pub rank: String,
    /// The term operator (`or` or `and`).
    pub operator: String,
    /// Distinct query terms after tokenization.
    pub query_terms: usize,
    /// Indexed documents available for ranking (corpus size).
    pub indexed_documents: usize,
    /// Candidate documents matched (present only for `EXPLAIN ANALYZE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidates: Option<usize>,
}

/// Hybrid clause summary in an EXPLAIN plan.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HybridPlan {
    /// The text field.
    pub text_field: String,
    /// The vector field.
    pub vector_field: String,
    /// The fusion mode (`weighted_sum` or `reciprocal_rank_fusion`).
    pub fusion: String,
    /// The text candidate source description.
    pub text_source: String,
    /// The vector candidate source description.
    pub vector_source: String,
    /// The text-signal weight.
    pub weight_text: f32,
    /// The vector-signal weight.
    pub weight_vector: f32,
    /// Requested fused result count.
    pub top_k: usize,
    /// Text candidate documents (present only for `EXPLAIN ANALYZE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_candidates: Option<usize>,
    /// Vector candidate documents (present only for `EXPLAIN ANALYZE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_candidates: Option<usize>,
}

/// A ranked candidate: a record id with its primary (or fused) score and, for
/// hybrid search, the component text and vector scores. Replaces the bare
/// `(RecordId, Option<f32>)` so component scores survive cursor paging.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scored {
    /// The record id.
    pub id: RecordId,
    /// The primary/fused relevance or similarity score, if ranked.
    pub score: Option<f32>,
    /// The BM25 text-relevance component, for hybrid results.
    pub text_score: Option<f32>,
    /// The vector-similarity component, for hybrid results.
    pub vector_score: Option<f32>,
}

impl Scored {
    /// An unranked candidate (equality lookup or full scan).
    pub fn plain(id: RecordId) -> Self {
        Scored {
            id,
            score: None,
            text_score: None,
            vector_score: None,
        }
    }

    /// A single-signal ranked candidate.
    pub fn ranked(id: RecordId, score: f32) -> Self {
        Scored {
            id,
            score: Some(score),
            text_score: None,
            vector_score: None,
        }
    }
}

/// The ordered result of planning a [`FindQuery`]: record ids with optional
/// vector scores, plus the plan. Ids are cheap to hold so cursors can page
/// without materializing every row up front.
pub struct PlannedFind {
    /// Ordered scored candidates after offset/limit.
    pub ordered: Vec<Scored>,
    /// The EXPLAIN plan.
    pub plan: ExplainPlan,
}

fn require_schema<'a>(ds: &'a dyn DataSource, collection: &str) -> Result<&'a CollectionSchema> {
    ds.schema(collection)
        .ok_or_else(|| Error::NotFound(format!("collection {collection}")))
}

/// Counts gathered while executing a find, surfaced by `EXPLAIN ANALYZE`.
struct ExecCounts {
    /// Candidate rows produced by the access path.
    scanned: usize,
    /// Rows that survived the residual filter.
    matched: usize,
    /// Rows returned after offset/limit.
    returned: usize,
    /// Time spent in the planner.
    planning: std::time::Duration,
}

/// The outcome of candidate selection for an access path: the candidate set
/// (with any per-record scores), whether results are score-ordered, and optional
/// EXPLAIN summaries for ranked-text and hybrid retrieval.
struct Selection {
    candidates: Vec<Scored>,
    score_ordered: bool,
    text_search: Option<TextSearchPlan>,
    hybrid: Option<HybridPlan>,
}

/// Resolve candidates (and any scores) for the planner's chosen access path.
fn select_candidates(
    ds: &dyn DataSource,
    indexes: &CollectionIndexes,
    query: &FindQuery,
    access: &Access,
) -> Result<Selection> {
    match access {
        Access::Vector { .. } => {
            let vs = query
                .vector
                .as_ref()
                .expect("vector access only chosen for a vector query");
            let metric = Metric::parse(&vs.metric)?;
            // Opt-in approximate (HNSW) preview when `vector_ann` is set; exact
            // search (the correctness baseline) otherwise.
            let neighbors = if let Some(ann) = &query.vector_ann {
                let params = HnswParams {
                    m: ann.m.unwrap_or(DEFAULT_ANN_M),
                    ef_construction: ann.ef_construction.unwrap_or(DEFAULT_ANN_EF_CONSTRUCTION),
                };
                let ef_search = ann.ef_search.unwrap_or(DEFAULT_ANN_EF_SEARCH).max(vs.k);
                indexes.vector_ann_nearest(&vs.field, &vs.query, vs.k, metric, params, ef_search)?
            } else {
                indexes.vector_nearest(&vs.field, &vs.query, vs.k, metric)?
            };
            let candidates = neighbors
                .into_iter()
                .map(|n| Scored::ranked(n.id, n.score))
                .collect();
            Ok(Selection {
                candidates,
                score_ordered: true,
                text_search: None,
                hybrid: None,
            })
        }
        Access::FullText { field, query: q } => {
            let results = indexes.text_search(field, q)?;
            let candidates = results
                .into_iter()
                .map(|(id, score)| Scored::ranked(id, score))
                .collect();
            Ok(Selection {
                candidates,
                score_ordered: true,
                text_search: None,
                hybrid: None,
            })
        }
        Access::TextRanked { field } => {
            let ts = query
                .text_search
                .as_ref()
                .expect("ranked-text access only chosen for a text_search query");
            let candidates = ranked_text_candidates(indexes, ts)?;
            let summary = TextSearchPlan {
                field: field.clone(),
                rank: match ts.rank {
                    crate::ir::TextRank::TermFrequency => "term_frequency".into(),
                    crate::ir::TextRank::Bm25 => "bm25".into(),
                },
                operator: if ts.operator.require_all() {
                    "and"
                } else {
                    "or"
                }
                .into(),
                query_terms: auradb_index::tokenize(&ts.query).len(),
                indexed_documents: indexes
                    .text_index_stats(field)
                    .map(|s| s.documents)
                    .unwrap_or(0),
                candidates: Some(candidates.len()),
            };
            Ok(Selection {
                candidates,
                score_ordered: true,
                text_search: Some(summary),
                hybrid: None,
            })
        }
        Access::Hybrid { .. } => {
            let hs = query
                .hybrid
                .as_ref()
                .expect("hybrid access only chosen for a hybrid query");
            let (candidates, text_n, vec_n) = hybrid_candidates(indexes, hs)?;
            let summary = HybridPlan {
                text_field: hs.text_field.clone(),
                vector_field: hs.vector_field.clone(),
                fusion: match hs.fusion {
                    crate::ir::FusionMode::ReciprocalRankFusion => "reciprocal_rank_fusion".into(),
                    crate::ir::FusionMode::WeightedSum => "weighted_sum".into(),
                },
                text_source: format!("bm25:{}", hs.text_field),
                vector_source: format!("exact_vector:{}", hs.vector_field),
                weight_text: hs.weights.text,
                weight_vector: hs.weights.vector,
                top_k: hs.top_k,
                text_candidates: Some(text_n),
                vector_candidates: Some(vec_n),
            };
            Ok(Selection {
                candidates,
                score_ordered: true,
                text_search: None,
                hybrid: Some(summary),
            })
        }
        Access::PointLookup { field, value }
        | Access::IndexLookup { field, value }
        | Access::DocumentPath { path: field, value } => Ok(Selection {
            candidates: indexes
                .lookup_eq(field, value)
                .unwrap_or_default()
                .into_iter()
                .map(Scored::plain)
                .collect(),
            score_ordered: false,
            text_search: None,
            hybrid: None,
        }),
        Access::Scan => Ok(Selection {
            candidates: ds
                .scan(&query.collection)
                .map(|r| Scored::plain(r.id))
                .collect(),
            score_ordered: false,
            text_search: None,
            hybrid: None,
        }),
    }
}

/// Map an [`Access`] to the public [`Strategy`] taxonomy.
fn strategy_for(access: &Access) -> Strategy {
    match access {
        Access::Vector { .. } => Strategy::VectorExactScan,
        Access::FullText { .. } => Strategy::FullTextScan,
        Access::TextRanked { .. } => Strategy::FullTextBm25,
        Access::Hybrid { .. } => Strategy::Hybrid,
        Access::PointLookup { .. } | Access::IndexLookup { .. } | Access::DocumentPath { .. } => {
            Strategy::IndexLookup
        }
        Access::Scan => Strategy::FullScan,
    }
}

/// Core find execution shared by [`execute_find`] and `EXPLAIN ANALYZE`: plans,
/// selects candidates, filters, orders, and applies offset/limit. Returns the
/// ordered ids, the plan, and execution counts.
///
/// `deadline` cooperatively bounds execution: candidate selection and the filter
/// loop poll it, and a query that runs past its budget returns a structured
/// [`Error::QueryTimeout`]. A [`Deadline::none`] (the default for callers that do
/// not set a timeout) never fires.
fn run_find(
    ds: &dyn DataSource,
    query: &FindQuery,
    deadline: &Deadline,
) -> Result<(PlannedFind, ExecCounts)> {
    query
        .validate_search_clauses()
        .map_err(Error::InvalidRequest)?;
    let schema = require_schema(ds, &query.collection)?;
    let indexes = ds
        .indexes(&query.collection)
        .ok_or_else(|| Error::Internal(format!("missing indexes for {}", query.collection)))?;
    let mut warnings = Vec::new();

    // 1. Plan: choose the access path by estimated cost.
    let plan_start = std::time::Instant::now();
    let stats = ds.stats(&query.collection);
    let live_row_count = match stats {
        Some(_) => 0, // unused when stats present
        None => ds.scan(&query.collection).count(),
    };
    let plan = planner::plan_find(query, schema, indexes, stats, live_row_count);
    let planning = plan_start.elapsed();

    // 2. Candidate selection per the chosen access path. Index-backed ranked
    // retrieval (BM25/vector/hybrid) runs inside the index; bound it by checking
    // the deadline immediately before and after selection.
    deadline.check()?;
    let selection = select_candidates(ds, indexes, query, &plan.access)?;
    deadline.check()?;
    let scanned = selection.candidates.len();
    if matches!(plan.access, Access::Scan) && scanned > 10_000 {
        warnings.push(format!("full scan of {scanned} records; consider an index"));
    }
    let score_ordered = selection.score_ordered;

    // 3. Filter candidates (always re-applied, even after an index seed). The
    // filter loop is the dominant cost on a full scan, so it polls the deadline.
    let mut matched: Vec<Scored> = Vec::new();
    for (i, cand) in selection.candidates.into_iter().enumerate() {
        deadline.check_at(i)?;
        let record = match ds.get(&query.collection, cand.id) {
            Some(r) => r,
            None => continue,
        };
        if let Some(filter) = &query.filter {
            if !eval::matches(record, filter) {
                continue;
            }
        }
        matched.push(cand);
    }
    let matched_count = matched.len();

    // 4. Ordering. A large sort can dominate a scan, so re-check before it.
    deadline.check()?;
    if score_ordered {
        matched.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then(a.id.cmp(&b.id))
        });
    } else if !query.order_by.is_empty() {
        order_records(ds, &query.collection, &mut matched, &query.order_by);
    }

    // 5. Offset / limit. A hybrid clause's `top_k` acts as the limit when no
    // explicit `limit` is set, so fusion+filter happen before the cut.
    let offset = query.offset.unwrap_or(0);
    let mut ordered: Vec<Scored> = matched.into_iter().skip(offset).collect();
    let effective_limit = query
        .limit
        .or_else(|| query.hybrid.as_ref().map(|h| h.top_k));
    if let Some(limit) = effective_limit {
        ordered.truncate(limit);
    }
    let returned = ordered.len();

    let explain = ExplainPlan {
        collection: query.collection.clone(),
        strategy: strategy_for(&plan.access),
        used_index: plan.used_index.clone(),
        estimated_candidates: scanned,
        filter_present: query.filter.is_some(),
        vector: query.vector.as_ref().map(|v| {
            let approximate = query.vector_ann.is_some();
            VectorPlan {
                field: v.field.clone(),
                k: v.k,
                metric: v.metric.clone(),
                // For exact search this is the brute-force scan size; for the
                // approximate preview the graph visits far fewer, so it is not a
                // meaningful "scanned" count and is omitted.
                vectors_scored: if approximate {
                    None
                } else {
                    indexes
                        .vector_field_stats()
                        .find(|(f, _, _)| *f == v.field)
                        .map(|(_, _, count)| count)
                },
                approximate,
                ef_search: query
                    .vector_ann
                    .as_ref()
                    .map(|a| a.ef_search.unwrap_or(DEFAULT_ANN_EF_SEARCH).max(v.k)),
            }
        }),
        text_search: selection.text_search,
        hybrid: selection.hybrid,
        order_by: query.order_by.clone(),
        includes: query.includes.clone(),
        warnings,
        estimated_rows: plan.estimated_rows,
        estimated_cost: plan.estimated_cost,
        plan_tree: Some(plan.node),
        analysis: None,
    };
    Ok((
        PlannedFind {
            ordered,
            plan: explain,
        },
        ExecCounts {
            scanned,
            matched: matched_count,
            returned,
            planning,
        },
    ))
}

/// Plan and run a find, returning ordered ids/scores and the EXPLAIN plan.
///
/// The query's [`FindQuery::timeout_ms`] bounds execution cooperatively; the
/// server clamps that field against its configured maximum before calling, so an
/// omitted timeout means "use the server default" (which may be unbounded).
pub fn execute_find(ds: &dyn DataSource, query: &FindQuery) -> Result<PlannedFind> {
    let deadline = Deadline::after_ms(query.timeout_ms.unwrap_or(0));
    Ok(run_find(ds, query, &deadline)?.0)
}

/// Plan and run a find under an explicit [`Deadline`], overriding the query's own
/// `timeout_ms`. Used by paths that resolve the effective budget separately.
pub fn execute_find_within(
    ds: &dyn DataSource,
    query: &FindQuery,
    deadline: &Deadline,
) -> Result<PlannedFind> {
    Ok(run_find(ds, query, deadline)?.0)
}

/// Resolve BM25-ranked text candidates for a `text_search` clause.
fn ranked_text_candidates(indexes: &CollectionIndexes, ts: &TextSearch) -> Result<Vec<Scored>> {
    if auradb_index::tokenize(&ts.query).is_empty() {
        return Err(Error::InvalidRequest(
            "text_search query has no searchable terms".into(),
        ));
    }
    let require_all = ts.operator.require_all();
    let results = match ts.rank {
        TextRank::TermFrequency => indexes.text_search(&ts.field, &ts.query)?,
        TextRank::Bm25 => {
            let k1 = ts.k1.unwrap_or(BM25_DEFAULT_K1);
            let b = ts.b.unwrap_or(BM25_DEFAULT_B);
            indexes.text_bm25_search(&ts.field, &ts.query, require_all, k1, b)?
        }
    };
    Ok(results
        .into_iter()
        .map(|(id, score)| Scored::ranked(id, score))
        .collect())
}

/// The reciprocal-rank-fusion smoothing constant. A larger value flattens the
/// influence of high ranks; 60 is the value from the original RRF paper.
const RRF_K: f32 = 60.0;

/// Resolve fused hybrid candidates plus the per-signal candidate counts.
fn hybrid_candidates(
    indexes: &CollectionIndexes,
    hs: &HybridSearch,
) -> Result<(Vec<Scored>, usize, usize)> {
    if auradb_index::tokenize(&hs.text_query).is_empty() {
        return Err(Error::InvalidRequest(
            "hybrid text query has no searchable terms".into(),
        ));
    }
    if !(hs.weights.text.is_finite() && hs.weights.vector.is_finite())
        || hs.weights.text < 0.0
        || hs.weights.vector < 0.0
        || (hs.weights.text == 0.0 && hs.weights.vector == 0.0)
    {
        return Err(Error::InvalidRequest(
            "hybrid weights must be non-negative and not both zero".into(),
        ));
    }
    if hs.top_k == 0 {
        return Err(Error::InvalidRequest("hybrid top_k must be >= 1".into()));
    }
    // Text signal (BM25). The structured dimension-mismatch error comes from the
    // vector index below; the text index validates its own field.
    let k1 = hs.k1.unwrap_or(BM25_DEFAULT_K1);
    let b = hs.b.unwrap_or(BM25_DEFAULT_B);
    let text = indexes.text_bm25_search(
        &hs.text_field,
        &hs.text_query,
        hs.operator.require_all(),
        k1,
        b,
    )?;
    // Vector signal (exact). Fetch a generous candidate pool so fusion has both
    // signals to combine, then truncate after fusion.
    let metric = Metric::parse(hs.metric_name())?;
    let pool = hs.top_k.saturating_mul(4).max(hs.top_k);
    let vectors = indexes.vector_nearest(&hs.vector_field, &hs.vector, pool, metric)?;

    let text_n = text.len();
    let vec_n = vectors.len();
    let text_map: std::collections::HashMap<RecordId, f32> = text.iter().copied().collect();
    let vec_map: std::collections::HashMap<RecordId, f32> =
        vectors.iter().map(|n| (n.id, n.score)).collect();

    // Deterministic candidate union ordered by id so fusion is reproducible.
    let mut ids: Vec<RecordId> = text_map.keys().chain(vec_map.keys()).copied().collect();
    ids.sort_unstable();
    ids.dedup();

    let fused: Vec<Scored> = match hs.fusion {
        FusionMode::WeightedSum => {
            let (tmin, tmax) = min_max(text.iter().map(|(_, s)| *s));
            let (vmin, vmax) = min_max(vectors.iter().map(|n| n.score));
            ids.iter()
                .map(|id| {
                    let t = text_map.get(id).copied();
                    let v = vec_map.get(id).copied();
                    let tn = t.map(|s| normalize(s, tmin, tmax)).unwrap_or(0.0);
                    let vn = v.map(|s| normalize(s, vmin, vmax)).unwrap_or(0.0);
                    Scored {
                        id: *id,
                        score: Some(hs.weights.text * tn + hs.weights.vector * vn),
                        text_score: t,
                        vector_score: v,
                    }
                })
                .collect()
        }
        FusionMode::ReciprocalRankFusion => {
            let text_rank = rank_map(&text);
            let vec_rank = rank_map(&vectors.iter().map(|n| (n.id, n.score)).collect::<Vec<_>>());
            ids.iter()
                .map(|id| {
                    let t_contrib = text_rank
                        .get(id)
                        .map(|r| hs.weights.text / (RRF_K + *r as f32))
                        .unwrap_or(0.0);
                    let v_contrib = vec_rank
                        .get(id)
                        .map(|r| hs.weights.vector / (RRF_K + *r as f32))
                        .unwrap_or(0.0);
                    Scored {
                        id: *id,
                        score: Some(t_contrib + v_contrib),
                        text_score: text_map.get(id).copied(),
                        vector_score: vec_map.get(id).copied(),
                    }
                })
                .collect()
        }
    };
    Ok((fused, text_n, vec_n))
}

/// Min and max of a score iterator, or `(0, 0)` when empty.
fn min_max(scores: impl Iterator<Item = f32>) -> (f32, f32) {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for s in scores {
        min = min.min(s);
        max = max.max(s);
    }
    if min.is_finite() {
        (min, max)
    } else {
        (0.0, 0.0)
    }
}

/// Min-max normalize `s` into `[0, 1]`; a zero-width range maps any present value
/// to `1.0` so a single candidate still counts.
fn normalize(s: f32, min: f32, max: f32) -> f32 {
    if (max - min).abs() < f32::EPSILON {
        1.0
    } else {
        (s - min) / (max - min)
    }
}

/// 1-based rank of each id in a descending-score list (input already ordered).
fn rank_map(scored: &[(RecordId, f32)]) -> std::collections::HashMap<RecordId, usize> {
    scored
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, i + 1))
        .collect()
}

fn order_records(ds: &dyn DataSource, collection: &str, matched: &mut [Scored], keys: &[OrderKey]) {
    matched.sort_by(|a, b| {
        let ra = ds.get(collection, a.id);
        let rb = ds.get(collection, b.id);
        for key in keys {
            let va = ra.and_then(|r| r.get_path(&key.field));
            let vb = rb.and_then(|r| r.get_path(&key.field));
            let ord = match (va, vb) {
                (Some(x), Some(y)) => eval::order(x, y).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            let ord = if key.desc { ord.reverse() } else { ord };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        a.id.cmp(&b.id)
    });
}

/// Materialize a page of rows (projection + relationship hydration + scores).
/// `start_rank` is the 0-based offset of this page within the full ordered result
/// so each row's reported `rank` is stable across cursor pages.
pub fn materialize_page(
    ds: &dyn DataSource,
    query: &FindQuery,
    page: &[Scored],
    start_rank: usize,
) -> Result<Vec<Row>> {
    let schema = require_schema(ds, &query.collection)?;
    // Ranked clauses (vector / text_search / hybrid) expose a 1-based rank.
    let ranked = query.vector.is_some() || query.text_search.is_some() || query.hybrid.is_some();
    let mut rows = Vec::with_capacity(page.len());
    for (offset, cand) in page.iter().enumerate() {
        let record = match ds.get(&query.collection, cand.id) {
            Some(r) => r,
            None => continue,
        };
        let fields = match &query.projection {
            Some(proj) => {
                let mut m = auradb_core::Document::new();
                for name in proj {
                    if let Some(v) = record.fields.get(name) {
                        m.insert(name.clone(), v.clone());
                    }
                }
                m
            }
            None => record.fields.clone(),
        };
        let mut includes = std::collections::BTreeMap::new();
        for rel_name in &query.includes {
            let rel = schema.relationship(rel_name).ok_or_else(|| {
                Error::InvalidRequest(format!(
                    "{rel_name} is not a relationship on {}",
                    query.collection
                ))
            })?;
            let related = hydrate(
                ds,
                rel.target.as_str(),
                rel.cardinality,
                record.get(rel_name),
            )?;
            includes.insert(rel_name.clone(), related);
        }
        rows.push(Row {
            id: cand.id.to_string(),
            fields,
            score: cand.score,
            text_score: cand.text_score,
            vector_score: cand.vector_score,
            rank: if ranked {
                Some(start_rank + offset + 1)
            } else {
                None
            },
            includes,
        });
    }
    Ok(rows)
}

/// Materialize the first page of a result (rank starts at 1). Kept for callers
/// that page from the beginning; cursors use [`materialize_page`] with an offset.
pub fn materialize(ds: &dyn DataSource, query: &FindQuery, page: &[Scored]) -> Result<Vec<Row>> {
    materialize_page(ds, query, page, 0)
}

fn hydrate(
    ds: &dyn DataSource,
    target: &str,
    cardinality: Cardinality,
    value: Option<&Value>,
) -> Result<Vec<auradb_core::Document>> {
    let mut out = Vec::new();
    let keys: Vec<&Value> = match (cardinality, value) {
        (_, None) | (_, Some(Value::Null)) => Vec::new(),
        (Cardinality::ToOne, Some(v)) => vec![v],
        (Cardinality::ToMany, Some(Value::Array(items))) => items.iter().collect(),
        _ => Vec::new(),
    };
    for key in keys {
        if let Some(rec) = ds.resolve_link(target, key) {
            out.push(rec.fields.clone());
        }
    }
    Ok(out)
}

/// Count records matching a query.
pub fn execute_count(ds: &dyn DataSource, query: &CountQuery) -> Result<usize> {
    require_schema(ds, &query.collection)?;
    let count = ds
        .scan(&query.collection)
        .filter(|r| {
            query
                .filter
                .as_ref()
                .map(|f| eval::matches(r, f))
                .unwrap_or(true)
        })
        .count();
    Ok(count)
}

/// Test whether any record matches a query.
pub fn execute_exists(ds: &dyn DataSource, query: &ExistsQuery) -> Result<bool> {
    require_schema(ds, &query.collection)?;
    Ok(ds.scan(&query.collection).any(|r| {
        query
            .filter
            .as_ref()
            .map(|f| eval::matches(r, f))
            .unwrap_or(true)
    }))
}

/// Produce an EXPLAIN plan without materializing rows.
pub fn explain(ds: &dyn DataSource, query: &FindQuery) -> Result<ExplainPlan> {
    Ok(execute_find(ds, query)?.plan)
}

/// Produce an `EXPLAIN ANALYZE` plan: run the query and attach measured
/// execution metrics (scanned/matched/returned rows and timings). `snapshot_ts`
/// is the transaction read timestamp when run within a transaction.
pub fn explain_analyze(
    ds: &dyn DataSource,
    query: &FindQuery,
    snapshot_ts: Option<u64>,
) -> Result<ExplainPlan> {
    let started = std::time::Instant::now();
    let deadline = Deadline::after_ms(query.timeout_ms.unwrap_or(0));
    let (planned, counts) = run_find(ds, query, &deadline)?;
    let execution_micros = started.elapsed().as_micros();
    let stats_version = ds.stats_version();
    // Statistics look stale or absent when there are none, or when rows exist but
    // no per-field cardinality has been recorded (the planner then falls back to
    // default selectivity). Correctness is unaffected; the cost choice may not be
    // optimal until `analyze` runs.
    let stale_stats = match ds.stats(&query.collection) {
        None => true,
        Some(s) => s.row_count > 0 && s.field_cardinality.is_empty(),
    };
    let mut plan = planned.plan;
    let selected_index_reason = Some(selection_reason(&plan));
    if stale_stats && !plan.warnings.iter().any(|w| w.contains("statistics")) {
        plan.warnings.push(
            "planner statistics are unavailable or stale; access path chosen from defaults".into(),
        );
    }
    plan.analysis = Some(ExplainAnalysis {
        scanned_rows: counts.scanned,
        matched_rows: counts.matched,
        returned_rows: counts.returned,
        execution_micros,
        planning_micros: counts.planning.as_micros(),
        snapshot_ts,
        estimated_rows: plan.estimated_rows,
        planner_stats_version: stats_version,
        selected_index_reason,
        stale_stats,
    });
    Ok(plan)
}

/// A short, human-readable explanation of why the planner chose its access path.
fn selection_reason(plan: &ExplainPlan) -> String {
    match (&plan.strategy, &plan.used_index) {
        (Strategy::IndexLookup, Some(idx)) => {
            format!("equality lookup seeded by index `{idx}`")
        }
        (Strategy::FullTextScan, Some(idx)) => {
            format!("full-text index `{idx}` serves the text query")
        }
        (Strategy::FullTextBm25, Some(idx)) => {
            format!("full-text index `{idx}` serves the BM25 ranked search")
        }
        (Strategy::Hybrid, Some(idx)) => {
            format!("hybrid fusion over `{idx}` (BM25 text + exact vector)")
        }
        (Strategy::VectorExactScan, Some(idx)) => {
            format!("vector index `{idx}` serves the nearest-neighbour search")
        }
        (Strategy::FullScan, _) => {
            "no usable index for the filter; full collection scan".to_string()
        }
        (strategy, _) => format!("{strategy:?} access path"),
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use auradb_core::ErrorCode;

    #[test]
    fn none_and_zero_never_fire() {
        assert!(!Deadline::none().is_enabled());
        assert!(!Deadline::after_ms(0).is_enabled());
        // Polling at many indices must stay Ok for a disabled deadline.
        let d = Deadline::none();
        for i in 0..10_000 {
            d.check_at(i).expect("disabled deadline never fires");
        }
        d.check().expect("disabled deadline never fires");
    }

    #[test]
    fn enabled_deadline_reports_query_timeout_after_budget() {
        // Start a 1ms budget, then sleep well past it. Because the deadline's
        // clock started before the sleep, the check is deterministic regardless
        // of host speed.
        let d = Deadline::after_ms(1);
        assert!(d.is_enabled());
        std::thread::sleep(Duration::from_millis(8));
        let err = d.check().expect_err("expired deadline must fire");
        assert_eq!(err.code(), ErrorCode::QueryTimeout);
        let msg = err.to_string();
        assert!(msg.contains("deadline"), "message names the budget: {msg}");
    }

    #[test]
    fn fresh_deadline_within_budget_does_not_fire() {
        // A generous budget must not trip for an instantaneous check.
        let d = Deadline::after_ms(60_000);
        d.check().expect("a fresh, generous deadline does not fire");
    }
}
