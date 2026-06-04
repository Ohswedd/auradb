//! Query execution: candidate selection, filtering, ordering, projection,
//! relationship hydration, and EXPLAIN planning.

use std::cmp::Ordering;

use auradb_core::{Cardinality, CollectionSchema, Error, Record, RecordId, Result, Value};
use auradb_index::{CollectionIndexes, Metric};

use crate::eval;
use crate::ir::{CountQuery, ExistsQuery, Filter, FindQuery, OrderKey, Row, VectorSearch};

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

/// Candidate-selection output: ids, optional vector scores, strategy, and the
/// index used (if any).
type Selection = (
    Vec<RecordId>,
    Option<std::collections::HashMap<RecordId, f32>>,
    Strategy,
    Option<String>,
);

fn require_schema<'a>(ds: &'a dyn DataSource, collection: &str) -> Result<&'a CollectionSchema> {
    ds.schema(collection)
        .ok_or_else(|| Error::NotFound(format!("collection {collection}")))
}

/// Find an indexed equality clause to seed candidate selection.
fn indexed_seed(filter: &Filter, indexes: &CollectionIndexes) -> Option<(String, Vec<RecordId>)> {
    match filter {
        Filter::Compare {
            field,
            op: crate::ir::CompareOp::Eq,
            value,
        } if indexes.has_equality_index(field) => indexes
            .lookup_eq(field, value)
            .map(|ids| (field.clone(), ids)),
        Filter::And { filters } => filters.iter().find_map(|f| indexed_seed(f, indexes)),
        _ => None,
    }
}

/// Find a full-text clause that can be seeded by an inverted index.
fn text_seed<'a>(filter: &'a Filter, indexes: &CollectionIndexes) -> Option<(&'a str, &'a str)> {
    match filter {
        Filter::ContainsText { field, query } if indexes.has_text_index(field) => {
            Some((field.as_str(), query.as_str()))
        }
        Filter::And { filters } => filters.iter().find_map(|f| text_seed(f, indexes)),
        _ => None,
    }
}

/// Plan and run a find, returning ordered ids/scores and the EXPLAIN plan.
pub fn execute_find(ds: &dyn DataSource, query: &FindQuery) -> Result<PlannedFind> {
    let schema = require_schema(ds, &query.collection)?;
    let indexes = ds
        .indexes(&query.collection)
        .ok_or_else(|| Error::Internal(format!("missing indexes for {}", query.collection)))?;
    let mut warnings = Vec::new();

    // 1. Candidate selection.
    let (candidates, scores, strategy, used_index): Selection = if let Some(vs) = &query.vector {
        let (ids, scores) = vector_candidates(indexes, vs)?;
        (
            ids,
            Some(scores),
            Strategy::VectorExactScan,
            Some(vs.field.clone()),
        )
    } else if let Some(filter) = &query.filter {
        if let Some((field, q)) = text_seed(filter, indexes) {
            let results = indexes.text_search(field, q)?;
            let mut ids = Vec::with_capacity(results.len());
            let mut scores = std::collections::HashMap::new();
            for (id, score) in results {
                ids.push(id);
                scores.insert(id, score);
            }
            (
                ids,
                Some(scores),
                Strategy::FullTextScan,
                Some(field.to_string()),
            )
        } else if let Some((field, ids)) = indexed_seed(filter, indexes) {
            (ids, None, Strategy::IndexLookup, Some(field))
        } else {
            let ids: Vec<RecordId> = ds.scan(&query.collection).map(|r| r.id).collect();
            if ids.len() > 10_000 {
                warnings.push(format!(
                    "full scan of {} records; consider an index",
                    ids.len()
                ));
            }
            (ids, None, Strategy::FullScan, None)
        }
    } else {
        let ids: Vec<RecordId> = ds.scan(&query.collection).map(|r| r.id).collect();
        (ids, None, Strategy::FullScan, None)
    };
    let estimated_candidates = candidates.len();
    // Vector and full-text selections carry per-record scores and are ordered by
    // descending score; other selections honor `order_by`.
    let score_ordered = scores.is_some();

    // 2. Filter candidates (always re-applied, even after an index seed).
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

    // 3. Ordering.
    if score_ordered {
        // Ordered by descending score (vector similarity or text relevance).
        matched.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
    } else if !query.order_by.is_empty() {
        order_records(ds, &query.collection, &mut matched, &query.order_by);
    }

    // 4. Offset / limit.
    let offset = query.offset.unwrap_or(0);
    let mut ordered: Vec<(RecordId, Option<f32>)> = matched.into_iter().skip(offset).collect();
    if let Some(limit) = query.limit {
        ordered.truncate(limit);
    }

    let plan = ExplainPlan {
        collection: query.collection.clone(),
        strategy,
        used_index,
        estimated_candidates,
        filter_present: query.filter.is_some(),
        vector: query.vector.as_ref().map(|v| VectorPlan {
            field: v.field.clone(),
            k: v.k,
            metric: v.metric.clone(),
        }),
        order_by: query.order_by.clone(),
        includes: query.includes.clone(),
        warnings,
    };
    let _ = schema; // schema validated; used for includes during materialize
    Ok(PlannedFind { ordered, plan })
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
