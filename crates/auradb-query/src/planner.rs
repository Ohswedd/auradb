//! The query planner: costed access-path selection.
//!
//! Given a [`FindQuery`], the collection schema, its indexes, and (optional)
//! [`CollectionStats`], the planner enumerates the viable access paths — point
//! lookup, secondary / document-path / full-text index lookups, vector search,
//! or a full scan — estimates the number of candidate rows and a cost for each
//! using row counts and per-field cardinality, and chooses the cheapest. The
//! result is a [`Plan`] carrying the chosen access path and a serializable plan
//! tree for `EXPLAIN`.
//!
//! This is genuine cost-based selection: the estimate is driven by statistics
//! (collection row count and equality selectivity = rows / distinct values), not
//! by mere index existence. With no statistics the planner falls back to a
//! default selectivity, still preferring a selective index over a full scan.

use auradb_core::CollectionSchema;
use auradb_index::CollectionIndexes;

use crate::ir::{CompareOp, Filter, FindQuery};
use crate::plan::{Access, Plan, PlanNode};
use crate::stats::CollectionStats;

/// Assumed equality selectivity when no cardinality statistic is available: a
/// non-unique equality predicate is assumed to match ~10% of the collection.
const DEFAULT_EQ_SELECTIVITY: f64 = 0.10;
/// Assumed full-text selectivity (fraction of documents matched) absent stats.
const DEFAULT_TEXT_SELECTIVITY: f64 = 0.05;

// Cost is modelled as the number of candidate records the access path must fetch
// and filter. A scan costs `row_count`; an index lookup costs its estimated
// candidate count (always ≤ `row_count`). An index is therefore never costed
// above a scan, and ties (e.g. a one-row collection, or a non-selective index)
// break toward the index because index candidates are enumerated first. The
// statistic-driven value of the planner is choosing the *most selective* index
// among several candidates, and choosing a scan when no index applies.

/// A candidate access path with its estimates, before final selection.
struct Candidate {
    access: Access,
    estimated_rows: usize,
    cost: f64,
}

/// Plan a [`FindQuery`] against `schema` / `indexes` / `stats`. `live_row_count`
/// is the planner's fallback collection size when no row-count statistic exists.
pub fn plan_find(
    query: &FindQuery,
    schema: &CollectionSchema,
    indexes: &CollectionIndexes,
    stats: Option<&CollectionStats>,
    live_row_count: usize,
) -> Plan {
    let row_count = stats.map(|s| s.row_count).unwrap_or(live_row_count);

    // A vector clause forces an exact vector search (the only way to satisfy it).
    let chosen = if let Some(vs) = &query.vector {
        let rows = stats
            .and_then(|s| s.vector_count.get(&vs.field).copied())
            .unwrap_or(row_count)
            .min(vs.k.max(1));
        Candidate {
            access: Access::Vector {
                field: vs.field.clone(),
                k: vs.k,
                metric: vs.metric.clone(),
            },
            estimated_rows: rows,
            // Exact search visits every indexed vector.
            cost: stats
                .and_then(|s| s.vector_count.get(&vs.field).copied())
                .unwrap_or(row_count) as f64,
        }
    } else {
        // Enumerate index-seeded candidates from the filter, plus the full scan
        // baseline, and pick the cheapest.
        let mut candidates: Vec<Candidate> = Vec::new();
        if let Some(filter) = &query.filter {
            collect_candidates(filter, schema, indexes, stats, row_count, &mut candidates);
        }
        candidates.push(Candidate {
            access: Access::Scan,
            estimated_rows: row_count,
            cost: row_count as f64,
        });
        candidates
            .into_iter()
            .min_by(|a, b| {
                a.cost
                    .partial_cmp(&b.cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("scan candidate always present")
    };

    let node = build_tree(query, &chosen);
    Plan {
        used_index: chosen.access.used_index(),
        access: chosen.access,
        estimated_rows: chosen.estimated_rows,
        estimated_cost: chosen.cost,
        node,
    }
}

/// Estimate rows and cost for a non-unique equality lookup on `field`.
fn eq_index_estimate(field: &str, stats: Option<&CollectionStats>, row_count: usize) -> usize {
    match stats
        .and_then(|s| s.cardinality(field))
        .and_then(|distinct| row_count.checked_div(distinct))
    {
        Some(rows) => rows.max(1),
        None => ((row_count as f64 * DEFAULT_EQ_SELECTIVITY).ceil() as usize).max(1),
    }
}

/// Recursively gather index-seeded access candidates from a filter. Only
/// conjunctions are descended into: an equality (or full-text) term under an
/// `And` can seed selection; `Or`/`Not` cannot (the residual filter still runs).
fn collect_candidates(
    filter: &Filter,
    schema: &CollectionSchema,
    indexes: &CollectionIndexes,
    stats: Option<&CollectionStats>,
    row_count: usize,
    out: &mut Vec<Candidate>,
) {
    match filter {
        Filter::Compare {
            field,
            op: CompareOp::Eq,
            value,
        } if indexes.has_equality_index(field) => {
            let is_unique = schema
                .fields
                .iter()
                .any(|f| &f.name == field && (f.primary_key || f.unique));
            let is_doc_path = schema.document_path_indexes().any(|p| p == field.as_str());
            if is_unique {
                out.push(Candidate {
                    access: Access::PointLookup {
                        field: field.clone(),
                        value: value.clone(),
                    },
                    estimated_rows: 1,
                    cost: 1.0,
                });
            } else if is_doc_path {
                let rows = eq_index_estimate(field, stats, row_count);
                out.push(Candidate {
                    access: Access::DocumentPath {
                        path: field.clone(),
                        value: value.clone(),
                    },
                    estimated_rows: rows,
                    cost: rows as f64,
                });
            } else {
                let rows = eq_index_estimate(field, stats, row_count);
                out.push(Candidate {
                    access: Access::IndexLookup {
                        field: field.clone(),
                        value: value.clone(),
                    },
                    estimated_rows: rows,
                    cost: rows as f64,
                });
            }
        }
        Filter::ContainsText { field, query } if indexes.has_text_index(field) => {
            let docs = stats.and_then(|s| s.text_field_docs.get(field).copied());
            let rows = match docs {
                Some(d) => ((d as f64 * DEFAULT_TEXT_SELECTIVITY).ceil() as usize).max(1),
                None => ((row_count as f64 * DEFAULT_TEXT_SELECTIVITY).ceil() as usize).max(1),
            };
            out.push(Candidate {
                access: Access::FullText {
                    field: field.clone(),
                    query: query.clone(),
                },
                estimated_rows: rows,
                cost: rows as f64,
            });
        }
        Filter::And { filters } => {
            for f in filters {
                collect_candidates(f, schema, indexes, stats, row_count, out);
            }
        }
        _ => {}
    }
}

/// Build the serializable plan tree: the access leaf wrapped by the query's
/// pipeline operators (filter, sort, offset, limit, includes, projection).
fn build_tree(query: &FindQuery, chosen: &Candidate) -> PlanNode {
    let rows = chosen.estimated_rows;
    let mut node = match &chosen.access {
        Access::PointLookup { field, .. } => PlanNode::PointLookup {
            index: field.clone(),
            estimated_rows: rows,
        },
        Access::IndexLookup { field, .. } => PlanNode::IndexLookup {
            index: field.clone(),
            field: field.clone(),
            estimated_rows: rows,
        },
        Access::DocumentPath { path, .. } => PlanNode::DocumentPathIndexLookup {
            index: path.clone(),
            path: path.clone(),
            estimated_rows: rows,
        },
        Access::FullText { field, .. } => PlanNode::FullTextIndexLookup {
            index: field.clone(),
            field: field.clone(),
            estimated_rows: rows,
        },
        Access::Vector { field, k, metric } => PlanNode::VectorSearch {
            field: field.clone(),
            k: *k,
            metric: metric.clone(),
            estimated_rows: rows,
        },
        Access::Scan => PlanNode::Scan {
            collection: query.collection.clone(),
            estimated_rows: rows,
        },
    };

    // Residual filter: the predicate is re-applied to every candidate regardless
    // of access path (an index seed never relies on the index alone).
    if query.filter.is_some() {
        node = PlanNode::Filter {
            input: Box::new(node),
            estimated_rows: rows,
        };
    }

    // Vector / full-text results are score-ordered; explicit order_by applies
    // otherwise.
    let score_ordered = matches!(
        chosen.access,
        Access::Vector { .. } | Access::FullText { .. }
    );
    if !score_ordered && !query.order_by.is_empty() {
        node = PlanNode::Sort {
            input: Box::new(node),
            keys: query.order_by.clone(),
        };
    }
    if let Some(offset) = query.offset {
        if offset > 0 {
            node = PlanNode::Offset {
                input: Box::new(node),
                offset,
            };
        }
    }
    if let Some(limit) = query.limit {
        node = PlanNode::Limit {
            input: Box::new(node),
            limit,
        };
    }
    if !query.includes.is_empty() {
        node = PlanNode::RelationshipInclude {
            input: Box::new(node),
            relationships: query.includes.clone(),
        };
    }
    if let Some(projection) = &query.projection {
        node = PlanNode::Projection {
            input: Box::new(node),
            fields: projection.clone(),
        };
    }
    node
}

/// Build a `Count` plan node over the access path for a filtered count.
pub fn plan_count_node(collection: &str, filtered: bool, estimated_rows: usize) -> PlanNode {
    let scan = PlanNode::Scan {
        collection: collection.to_string(),
        estimated_rows,
    };
    let input = if filtered {
        PlanNode::Filter {
            input: Box::new(scan),
            estimated_rows,
        }
    } else {
        scan
    };
    PlanNode::Count {
        input: Box::new(input),
    }
}

/// Build an `Exists` plan node over the access path.
pub fn plan_exists_node(collection: &str, filtered: bool, estimated_rows: usize) -> PlanNode {
    let scan = PlanNode::Scan {
        collection: collection.to_string(),
        estimated_rows,
    };
    let input = if filtered {
        PlanNode::Filter {
            input: Box::new(scan),
            estimated_rows,
        }
    } else {
        scan
    };
    PlanNode::Exists {
        input: Box::new(input),
    }
}

/// Build a `Mutation` plan node.
pub fn plan_mutation_node(kind: &str, collection: &str) -> PlanNode {
    PlanNode::Mutation {
        kind: kind.to_string(),
        collection: collection.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{OrderKey, VectorSearch};
    use crate::stats::CollectionStats;
    use auradb_core::{CollectionSchema, FieldDef, FieldType, IndexDef, IndexKind, Value};
    use auradb_index::CollectionIndexes;

    fn schema() -> CollectionSchema {
        CollectionSchema::new("Doc")
            .with_field(FieldDef {
                name: "id".into(),
                field_type: FieldType::Uuid,
                primary_key: true,
                unique: true,
                nullable: false,
                indexed: false,
            })
            .with_field(FieldDef {
                name: "status".into(),
                field_type: FieldType::String,
                primary_key: false,
                unique: false,
                nullable: true,
                indexed: true,
            })
            .with_field(FieldDef::new("title", FieldType::String))
            .with_field(FieldDef::new("metadata", FieldType::Document))
            .with_field(FieldDef::new("body", FieldType::String))
            .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
            .with_index(IndexDef {
                path: "metadata.source".into(),
                kind: IndexKind::DocumentPath,
            })
            .with_index(IndexDef {
                path: "body".into(),
                kind: IndexKind::FullText,
            })
    }

    fn idx() -> CollectionIndexes {
        CollectionIndexes::from_schema(&schema())
    }

    fn eq(field: &str, value: Value) -> Filter {
        Filter::Compare {
            field: field.into(),
            op: CompareOp::Eq,
            value,
        }
    }

    fn stats_with(row_count: usize, field: &str, distinct: usize) -> CollectionStats {
        let mut s = CollectionStats {
            row_count,
            ..Default::default()
        };
        s.field_cardinality.insert(field.into(), distinct);
        s
    }

    #[test]
    fn planner_prefers_primary_key_lookup() {
        let mut q = FindQuery::new("Doc");
        q.filter = Some(eq("id", Value::Text("x".into())));
        let plan = plan_find(&q, &schema(), &idx(), None, 1000);
        assert!(matches!(plan.access, Access::PointLookup { .. }));
        assert_eq!(plan.estimated_rows, 1);
    }

    #[test]
    fn planner_prefers_secondary_index_for_selective_filter() {
        let mut q = FindQuery::new("Doc");
        q.filter = Some(eq("status", Value::Text("published".into())));
        let stats = stats_with(1000, "status", 100);
        let plan = plan_find(&q, &schema(), &idx(), Some(&stats), 0);
        assert!(matches!(plan.access, Access::IndexLookup { .. }));
        assert_eq!(plan.estimated_rows, 10); // 1000 / 100
        assert!(plan.estimated_cost < 1000.0);
    }

    #[test]
    fn planner_prefers_doc_path_index() {
        let mut q = FindQuery::new("Doc");
        q.filter = Some(eq("metadata.source", Value::Text("import".into())));
        let plan = plan_find(&q, &schema(), &idx(), None, 1000);
        assert!(matches!(plan.access, Access::DocumentPath { .. }));
    }

    #[test]
    fn planner_prefers_full_text_index() {
        let mut q = FindQuery::new("Doc");
        q.filter = Some(Filter::ContainsText {
            field: "body".into(),
            query: "hello".into(),
        });
        let plan = plan_find(&q, &schema(), &idx(), None, 1000);
        assert!(matches!(plan.access, Access::FullText { .. }));
    }

    #[test]
    fn planner_uses_vector_search_for_nearest() {
        let mut q = FindQuery::new("Doc");
        q.vector = Some(VectorSearch {
            field: "embedding".into(),
            query: vec![1.0, 0.0, 0.0],
            k: 5,
            metric: "cosine".into(),
        });
        let plan = plan_find(&q, &schema(), &idx(), None, 1000);
        assert!(matches!(plan.access, Access::Vector { .. }));
    }

    #[test]
    fn planner_falls_back_to_scan_when_no_index() {
        let mut q = FindQuery::new("Doc");
        q.filter = Some(eq("title", Value::Text("untitled".into()))); // not indexed
        let plan = plan_find(&q, &schema(), &idx(), None, 1000);
        assert!(matches!(plan.access, Access::Scan));
        assert_eq!(plan.used_index, None);
    }

    #[test]
    fn planner_applies_sort_limit_projection_nodes() {
        let mut q = FindQuery::new("Doc");
        q.filter = Some(eq("status", Value::Text("a".into())));
        q.order_by = vec![OrderKey {
            field: "title".into(),
            desc: false,
        }];
        q.limit = Some(10);
        q.offset = Some(2);
        q.projection = Some(vec!["id".into()]);
        q.includes = vec![];
        let plan = plan_find(&q, &schema(), &idx(), None, 1000);
        // Outermost is projection -> limit -> offset -> sort -> filter -> access.
        let PlanNode::Projection { input, .. } = &plan.node else {
            panic!("expected projection root, got {:?}", plan.node);
        };
        let PlanNode::Limit { input, .. } = input.as_ref() else {
            panic!("expected limit");
        };
        let PlanNode::Offset { input, .. } = input.as_ref() else {
            panic!("expected offset");
        };
        let PlanNode::Sort { input, .. } = input.as_ref() else {
            panic!("expected sort");
        };
        assert!(matches!(input.as_ref(), PlanNode::Filter { .. }));
    }

    #[test]
    fn planner_cost_changes_after_analyze() {
        let mut q = FindQuery::new("Doc");
        q.filter = Some(eq("status", Value::Text("a".into())));
        // Before analyze: no stats, default selectivity over 1000 rows -> 100.
        let before = plan_find(&q, &schema(), &idx(), None, 1000);
        // After analyze: real cardinality of 50 distinct -> 20.
        let stats = stats_with(1000, "status", 50);
        let after = plan_find(&q, &schema(), &idx(), Some(&stats), 0);
        assert_eq!(before.estimated_rows, 100);
        assert_eq!(after.estimated_rows, 20);
        assert_ne!(before.estimated_cost, after.estimated_cost);
    }
}
