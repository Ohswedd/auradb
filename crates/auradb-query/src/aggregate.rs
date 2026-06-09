//! Aggregations and terms facets over a collection (v1.2.0).
//!
//! Aggregations (`count`, `min`, `max`) and terms facets are computed over a
//! single matched set: either a full scan + residual filter, or — when a
//! ranked-text clause is present — the BM25 candidate set (a "search facet").
//! A terms facet over an equality-indexed field with no residual filter is
//! served straight from the index's posting-list lengths (no record scan); every
//! other shape falls back to an honest scan. Bucket ordering is deterministic:
//! descending count, then ascending value, so `limit` truncation is stable.

use std::cmp::Ordering;
use std::collections::HashMap;

use auradb_core::{Error, Record, Result, Value};
use auradb_index::CollectionIndexes;

use crate::eval;
use crate::exec::{DataSource, Deadline};
use crate::ir::{AggregateMetric, AggregateOp, AggregateQuery, FacetRequest};

/// One computed aggregation metric.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MetricValue {
    /// The operator name (`count`, `min`, `max`).
    pub op: String,
    /// The field the operator applied to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// The computed value. `count` is an integer; `min`/`max` carry the field's
    /// value (or null when the matched set had no orderable value for it).
    pub value: Value,
}

/// One terms-facet bucket: a distinct field value and its count.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FacetBucket {
    /// The facet value.
    pub value: Value,
    /// The number of matched records carrying this value.
    pub count: usize,
}

/// The computed buckets for one faceted field.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FacetValues {
    /// The faceted field.
    pub field: String,
    /// Whether the buckets were served from an equality index (true) or a scan
    /// of the matched set (false). Reported honestly for diagnostics.
    pub used_index: bool,
    /// The buckets, ordered by descending count then ascending value, truncated
    /// to the request's limit.
    pub buckets: Vec<FacetBucket>,
}

/// One GROUP BY bucket: a distinct group-key value, the number of records in the
/// group, and the requested metrics recomputed over just that group.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GroupBucket {
    /// The distinct value of the group-by field for this bucket.
    pub key: Value,
    /// The number of matched records in this group.
    pub count: usize,
    /// The requested metrics computed over this group's records, in request
    /// order. Empty when the query requested no metrics (the `count` above still
    /// stands on its own).
    pub metrics: Vec<MetricValue>,
}

/// The result of a GROUP BY clause: one bucket per distinct group-key value
/// within the matched set, ordered by descending count then ascending key.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GroupByResult {
    /// The group-by field.
    pub field: String,
    /// The returned buckets, ordered by descending count then ascending key and
    /// truncated to the effective group limit.
    pub groups: Vec<GroupBucket>,
    /// The total number of distinct groups before truncation. When this exceeds
    /// `groups.len()`, the result was truncated by the group limit.
    pub group_count_total: usize,
    /// The effective group limit applied.
    pub group_limit: usize,
}

/// The result of an [`AggregateQuery`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AggregateResult {
    /// The collection aggregated.
    pub collection: String,
    /// Records that matched the filter / candidate set.
    pub matched: usize,
    /// Candidate records examined before the residual filter (the scan/candidate
    /// size). Equals `matched` when there is no filter.
    pub scanned: usize,
    /// Whether a residual filter was applied.
    pub filter_present: bool,
    /// Whether facets/metrics were scoped to a BM25 candidate set.
    pub search_scoped: bool,
    /// The computed metrics, in request order.
    pub metrics: Vec<MetricValue>,
    /// The computed facets, in request order.
    pub facets: Vec<FacetValues>,
    /// The GROUP BY result, when the query carried a `group_by` clause. Additive:
    /// omitted from the wire when absent, so older connectors are unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<GroupByResult>,
}

fn require_schema<'a>(
    ds: &'a dyn DataSource,
    collection: &str,
) -> Result<&'a auradb_core::CollectionSchema> {
    ds.schema(collection)
        .ok_or_else(|| Error::NotFound(format!("collection {collection}")))
}

/// Reject a facet/metric field that is a declared non-scalar (vector) field or
/// is neither a declared field nor a dotted document path. Dotted paths and
/// scalar fields are accepted; per-value non-scalar contents are rejected during
/// bucketing.
fn validate_field(schema: &auradb_core::CollectionSchema, field: &str, what: &str) -> Result<()> {
    if let Some(def) = schema.fields.iter().find(|f| f.name == field) {
        if matches!(def.field_type, auradb_core::FieldType::Vector { .. }) {
            return Err(Error::InvalidRequest(format!(
                "{what} field `{field}` is a vector; {what}s require a scalar field"
            )));
        }
        return Ok(());
    }
    // Unknown top-level field with no path separator is rejected; a dotted path
    // may resolve into a nested object at runtime.
    if !field.contains('.') {
        return Err(Error::InvalidRequest(format!(
            "{what} field `{field}` is not a field of collection `{}`",
            schema.name
        )));
    }
    Ok(())
}

/// A canonical, type-tagged bucket key for a scalar value. `None` for null
/// (skipped) and an error for non-scalar values (vector/array/object/bytes).
fn scalar_key(value: &Value) -> Result<Option<String>> {
    let key = match value {
        Value::Null => return Ok(None),
        Value::Bool(b) => format!("b:{b}"),
        Value::Int(i) => format!("i:{i}"),
        // Bit pattern keeps distinct floats distinct and NaN self-consistent.
        Value::Float(f) => format!("f:{:016x}", f.to_bits()),
        Value::Text(s) => format!("s:{s}"),
        Value::Timestamp(t) => format!("t:{t}"),
        Value::Bytes(_) | Value::Vector(_) | Value::Array(_) | Value::Object(_) => {
            return Err(Error::InvalidRequest(format!(
                "cannot facet on a {} value; facets require scalar fields",
                value.type_name()
            )))
        }
    };
    Ok(Some(key))
}

/// Deterministic bucket ordering: descending count, then ascending value, with a
/// stable type-name + key tiebreak when values are not mutually orderable.
fn order_buckets(a: &FacetBucket, b: &FacetBucket) -> Ordering {
    b.count.cmp(&a.count).then_with(|| {
        eval::order(&a.value, &b.value).unwrap_or_else(|| {
            a.value.type_name().cmp(b.value.type_name()).then_with(|| {
                scalar_key(&a.value)
                    .ok()
                    .flatten()
                    .cmp(&scalar_key(&b.value).ok().flatten())
            })
        })
    })
}

/// Execute an aggregation/facet query under a cooperative deadline.
pub fn execute_aggregate(
    ds: &dyn DataSource,
    q: &AggregateQuery,
    deadline: &Deadline,
) -> Result<AggregateResult> {
    q.validate().map_err(Error::InvalidRequest)?;
    let schema = require_schema(ds, &q.collection)?;
    let indexes = ds
        .indexes(&q.collection)
        .ok_or_else(|| Error::Internal(format!("missing indexes for {}", q.collection)))?;

    for m in &q.metrics {
        if let Some(field) = &m.field {
            validate_field(schema, field, "aggregation")?;
        }
    }
    for f in &q.facets {
        validate_field(schema, &f.field, "facet")?;
    }
    if let Some(field) = &q.group_by {
        validate_field(schema, field, "group_by")?;
    }

    // 1. Resolve the candidate id set: a BM25 candidate set (search facet) or a
    //    full scan. The residual filter is applied below in both cases.
    let search_scoped = q.text_search.is_some();
    let candidate_ids: Vec<auradb_core::RecordId> = if let Some(ts) = &q.text_search {
        deadline.check()?;
        let require_all = ts.operator.require_all();
        let results = match ts.rank {
            crate::ir::TextRank::TermFrequency => indexes.text_search(&ts.field, &ts.query)?,
            crate::ir::TextRank::Bm25 => {
                let k1 = ts.k1.unwrap_or(crate::ir::BM25_DEFAULT_K1);
                let b = ts.b.unwrap_or(crate::ir::BM25_DEFAULT_B);
                indexes.text_bm25_search(&ts.field, &ts.query, require_all, k1, b)?
            }
        };
        results.into_iter().map(|(id, _)| id).collect()
    } else {
        ds.scan(&q.collection).map(|r| r.id).collect()
    };
    let scanned = candidate_ids.len();

    // 2. Resolve matched records (residual filter applied), polling the deadline.
    let mut matched: Vec<&Record> = Vec::new();
    for (i, id) in candidate_ids.iter().enumerate() {
        deadline.check_at(i)?;
        let record = match ds.get(&q.collection, *id) {
            Some(r) => r,
            None => continue,
        };
        if let Some(filter) = &q.filter {
            if !eval::matches(record, filter) {
                continue;
            }
        }
        matched.push(record);
    }
    let matched_count = matched.len();

    // 3. Metrics.
    let metrics = q
        .metrics
        .iter()
        .map(|m| compute_metric(m, &matched, matched_count))
        .collect();

    // 4. Facets.
    let can_use_index_base = q.filter.is_none() && !search_scoped;
    let mut facets = Vec::with_capacity(q.facets.len());
    for f in &q.facets {
        deadline.check()?;
        facets.push(compute_facet(
            ds,
            indexes,
            &q.collection,
            f,
            &matched,
            can_use_index_base,
        )?);
    }

    // 5. GROUP BY (over the same matched set, so it composes with filters and
    //    BM25 candidate scoping for free).
    let groups = match &q.group_by {
        Some(field) => Some(compute_groups(field, &q.metrics, &matched, q, deadline)?),
        None => None,
    };

    Ok(AggregateResult {
        collection: q.collection.clone(),
        matched: matched_count,
        scanned,
        filter_present: q.filter.is_some(),
        search_scoped,
        metrics,
        facets,
        groups,
    })
}

/// Group the matched records by the distinct scalar value of `field`, computing
/// the requested `metrics` per group. Records whose group value is null or
/// missing are excluded. Groups are ordered by descending count then ascending
/// key, then truncated to the effective group limit; the full distinct-group
/// count is reported so truncation is visible.
fn compute_groups(
    field: &str,
    metrics: &[AggregateMetric],
    matched: &[&Record],
    q: &AggregateQuery,
    deadline: &Deadline,
) -> Result<GroupByResult> {
    // key -> (representative typed value, member records)
    let mut groups: HashMap<String, (Value, Vec<&Record>)> = HashMap::new();
    for (i, rec) in matched.iter().enumerate() {
        deadline.check_at(i)?;
        let Some(value) = rec.get_path(field) else {
            continue;
        };
        let Some(key) = scalar_key(value)? else {
            continue;
        };
        groups
            .entry(key)
            .and_modify(|(_, members)| members.push(rec))
            .or_insert_with(|| (value.clone(), vec![rec]));
    }

    let group_count_total = groups.len();
    let mut buckets: Vec<GroupBucket> = groups
        .into_values()
        .map(|(key, members)| {
            let count = members.len();
            let group_metrics = metrics
                .iter()
                .map(|m| compute_metric(m, &members, count))
                .collect();
            GroupBucket {
                key,
                count,
                metrics: group_metrics,
            }
        })
        .collect();

    // Deterministic: descending count, ascending key, with a stable type-name +
    // key tiebreak when keys are not mutually orderable.
    buckets.sort_by(|a, b| {
        b.count.cmp(&a.count).then_with(|| {
            eval::order(&a.key, &b.key).unwrap_or_else(|| {
                a.key.type_name().cmp(b.key.type_name()).then_with(|| {
                    scalar_key(&a.key)
                        .ok()
                        .flatten()
                        .cmp(&scalar_key(&b.key).ok().flatten())
                })
            })
        })
    });

    let group_limit = q.effective_group_limit();
    buckets.truncate(group_limit);

    Ok(GroupByResult {
        field: field.to_string(),
        groups: buckets,
        group_count_total,
        group_limit,
    })
}

/// Coerce a scalar value to `f64` for numeric aggregation. `Int`/`Float` only;
/// everything else (including null, text, timestamp) yields `None` and is skipped.
fn as_numeric(value: &Value) -> Option<f64> {
    match value {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

fn compute_metric(m: &AggregateMetric, matched: &[&Record], matched_count: usize) -> MetricValue {
    let value = match m.op {
        AggregateOp::Count => Value::Int(matched_count as i64),
        AggregateOp::Avg => {
            let field = m.field.as_deref().unwrap_or_default();
            let mut sum = 0.0_f64;
            let mut n = 0_u64;
            for rec in matched {
                let Some(v) = rec.get_path(field) else {
                    continue;
                };
                if let Some(x) = as_numeric(v) {
                    sum += x;
                    n += 1;
                }
            }
            if n == 0 {
                Value::Null
            } else {
                Value::Float(sum / n as f64)
            }
        }
        AggregateOp::Min | AggregateOp::Max => {
            let field = m.field.as_deref().unwrap_or_default();
            let want_min = matches!(m.op, AggregateOp::Min);
            let mut best: Option<Value> = None;
            for rec in matched {
                let Some(v) = rec.get_path(field) else {
                    continue;
                };
                if v.is_null() {
                    continue;
                }
                best = Some(match best {
                    None => v.clone(),
                    Some(cur) => match eval::order(v, &cur) {
                        Some(Ordering::Less) if want_min => v.clone(),
                        Some(Ordering::Greater) if !want_min => v.clone(),
                        _ => cur,
                    },
                });
            }
            best.unwrap_or(Value::Null)
        }
    };
    MetricValue {
        op: m.op.name().to_string(),
        field: m.field.clone(),
        value,
    }
}

fn compute_facet(
    ds: &dyn DataSource,
    indexes: &CollectionIndexes,
    collection: &str,
    f: &FacetRequest,
    matched: &[&Record],
    can_use_index_base: bool,
) -> Result<FacetValues> {
    let use_index = can_use_index_base && indexes.has_equality_index(&f.field);

    // key -> (representative typed value, count)
    let mut groups: HashMap<String, (Value, usize)> = HashMap::new();

    if use_index {
        // Index path: posting-list lengths give counts directly; a representative
        // record recovers the typed value for each distinct key.
        let postings = indexes
            .facet_postings(&f.field)
            .expect("has_equality_index implies facet_postings is Some");
        for (rep_id, count) in postings {
            let Some(record) = ds.get(collection, rep_id) else {
                continue;
            };
            let Some(value) = record.get_path(&f.field) else {
                continue;
            };
            let Some(key) = scalar_key(value)? else {
                continue;
            };
            groups
                .entry(key)
                .and_modify(|(_, c)| *c += count)
                .or_insert((value.clone(), count));
        }
    } else {
        // Scan path over the matched set.
        for rec in matched {
            let Some(value) = rec.get_path(&f.field) else {
                continue;
            };
            let Some(key) = scalar_key(value)? else {
                continue;
            };
            groups
                .entry(key)
                .and_modify(|(_, c)| *c += 1)
                .or_insert((value.clone(), 1));
        }
    }

    let mut buckets: Vec<FacetBucket> = groups
        .into_values()
        .map(|(value, count)| FacetBucket { value, count })
        .collect();
    buckets.sort_by(order_buckets);
    buckets.truncate(f.effective_limit());

    Ok(FacetValues {
        field: f.field.clone(),
        used_index: use_index,
        buckets,
    })
}
