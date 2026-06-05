//! Query execution: candidate selection, filtering, ordering, projection,
//! relationship hydration, and EXPLAIN planning.

use std::cmp::Ordering;

use auradb_core::{Cardinality, CollectionSchema, Error, Record, RecordId, Result, Value};
use auradb_index::{CollectionIndexes, Metric};

use crate::eval;
use crate::ir::{CountQuery, ExistsQuery, FindQuery, OrderKey, Row, VectorSearch};
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
    /// Full-text candidate selection seeded by an inverted index.
    FullTextScan,
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
}

/// The ordered result of planning a [`FindQuery`]: record ids with optional
/// vector scores, plus the plan. Ids are cheap to hold so cursors can page
/// without materializing every row up front.
pub struct PlannedFind {
    /// Ordered `(record id, score)` after offset/limit.
    pub ordered: Vec<(RecordId, Option<f32>)>,
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

/// Candidate ids plus optional per-record scores (vector similarity / text
/// relevance) from an access path.
type Candidates = (
    Vec<RecordId>,
    Option<std::collections::HashMap<RecordId, f32>>,
);

/// Resolve candidate ids (and optional scores) for the planner's chosen access
/// path, executed against `ds`/`indexes`.
fn select_candidates(
    ds: &dyn DataSource,
    indexes: &CollectionIndexes,
    query: &FindQuery,
    access: &Access,
) -> Result<Candidates> {
    match access {
        Access::Vector { .. } => {
            let vs = query
                .vector
                .as_ref()
                .expect("vector access only chosen for a vector query");
            let (ids, scores) = vector_candidates(indexes, vs)?;
            Ok((ids, Some(scores)))
        }
        Access::FullText { field, query: q } => {
            let results = indexes.text_search(field, q)?;
            let mut ids = Vec::with_capacity(results.len());
            let mut scores = std::collections::HashMap::new();
            for (id, score) in results {
                ids.push(id);
                scores.insert(id, score);
            }
            Ok((ids, Some(scores)))
        }
        Access::PointLookup { field, value }
        | Access::IndexLookup { field, value }
        | Access::DocumentPath { path: field, value } => {
            Ok((indexes.lookup_eq(field, value).unwrap_or_default(), None))
        }
        Access::Scan => Ok((ds.scan(&query.collection).map(|r| r.id).collect(), None)),
    }
}

/// Map an [`Access`] to the public [`Strategy`] taxonomy.
fn strategy_for(access: &Access) -> Strategy {
    match access {
        Access::Vector { .. } => Strategy::VectorExactScan,
        Access::FullText { .. } => Strategy::FullTextScan,
        Access::PointLookup { .. } | Access::IndexLookup { .. } | Access::DocumentPath { .. } => {
            Strategy::IndexLookup
        }
        Access::Scan => Strategy::FullScan,
    }
}

/// Core find execution shared by [`execute_find`] and `EXPLAIN ANALYZE`: plans,
/// selects candidates, filters, orders, and applies offset/limit. Returns the
/// ordered ids, the plan, and execution counts.
fn run_find(ds: &dyn DataSource, query: &FindQuery) -> Result<(PlannedFind, ExecCounts)> {
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

    // 2. Candidate selection per the chosen access path.
    let (candidates, scores) = select_candidates(ds, indexes, query, &plan.access)?;
    let scanned = candidates.len();
    if matches!(plan.access, Access::Scan) && scanned > 10_000 {
        warnings.push(format!("full scan of {scanned} records; consider an index"));
    }
    // Vector and full-text selections carry per-record scores and are ordered by
    // descending score; other selections honor `order_by`.
    let score_ordered = scores.is_some();

    // 3. Filter candidates (always re-applied, even after an index seed).
    let mut matched: Vec<(RecordId, Option<f32>)> = Vec::new();
    for id in candidates {
        let record = match ds.get(&query.collection, id) {
            Some(r) => r,
            None => continue,
        };
        if let Some(filter) = &query.filter {
            if !eval::matches(record, filter) {
                continue;
            }
        }
        let score = scores.as_ref().and_then(|m| m.get(&id).copied());
        matched.push((id, score));
    }
    let matched_count = matched.len();

    // 4. Ordering.
    if score_ordered {
        matched.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
    } else if !query.order_by.is_empty() {
        order_records(ds, &query.collection, &mut matched, &query.order_by);
    }

    // 5. Offset / limit.
    let offset = query.offset.unwrap_or(0);
    let mut ordered: Vec<(RecordId, Option<f32>)> = matched.into_iter().skip(offset).collect();
    if let Some(limit) = query.limit {
        ordered.truncate(limit);
    }
    let returned = ordered.len();

    let explain = ExplainPlan {
        collection: query.collection.clone(),
        strategy: strategy_for(&plan.access),
        used_index: plan.used_index.clone(),
        estimated_candidates: scanned,
        filter_present: query.filter.is_some(),
        vector: query.vector.as_ref().map(|v| VectorPlan {
            field: v.field.clone(),
            k: v.k,
            metric: v.metric.clone(),
        }),
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
pub fn execute_find(ds: &dyn DataSource, query: &FindQuery) -> Result<PlannedFind> {
    Ok(run_find(ds, query)?.0)
}

fn vector_candidates(
    indexes: &CollectionIndexes,
    vs: &VectorSearch,
) -> Result<(Vec<RecordId>, std::collections::HashMap<RecordId, f32>)> {
    let metric = Metric::parse(&vs.metric)?;
    let neighbors = indexes.vector_nearest(&vs.field, &vs.query, vs.k, metric)?;
    let mut ids = Vec::with_capacity(neighbors.len());
    let mut scores = std::collections::HashMap::new();
    for n in neighbors {
        ids.push(n.id);
        scores.insert(n.id, n.score);
    }
    Ok((ids, scores))
}

fn order_records(
    ds: &dyn DataSource,
    collection: &str,
    matched: &mut [(RecordId, Option<f32>)],
    keys: &[OrderKey],
) {
    matched.sort_by(|a, b| {
        let ra = ds.get(collection, a.0);
        let rb = ds.get(collection, b.0);
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
        a.0.cmp(&b.0)
    });
}

/// Materialize a page of rows (projection + relationship hydration + score).
pub fn materialize(
    ds: &dyn DataSource,
    query: &FindQuery,
    page: &[(RecordId, Option<f32>)],
) -> Result<Vec<Row>> {
    let schema = require_schema(ds, &query.collection)?;
    let mut rows = Vec::with_capacity(page.len());
    for (id, score) in page {
        let record = match ds.get(&query.collection, *id) {
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
            id: id.to_string(),
            fields,
            score: *score,
            includes,
        });
    }
    Ok(rows)
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
    let (planned, counts) = run_find(ds, query)?;
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
        (Strategy::VectorExactScan, Some(idx)) => {
            format!("vector index `{idx}` serves the nearest-neighbour search")
        }
        (Strategy::FullScan, _) => {
            "no usable index for the filter; full collection scan".to_string()
        }
        (strategy, _) => format!("{strategy:?} access path"),
    }
}
