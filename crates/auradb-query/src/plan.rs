//! Query plan representation: the access path chosen by the planner plus the
//! pipeline of operators applied on top, rendered as a serializable plan tree
//! for `EXPLAIN`.

use auradb_core::Value;
use serde::{Deserialize, Serialize};

use crate::ir::OrderKey;

/// The candidate access path the planner can choose to seed candidate selection.
/// Carries the data execution needs (lookup values); the serialized [`PlanNode`]
/// tree exposes only field/index names and estimates.
#[derive(Debug, Clone, PartialEq)]
pub enum Access {
    /// Primary-key / unique equality: resolves to at most one record.
    PointLookup {
        /// The field (primary key or unique).
        field: String,
        /// The equality value.
        value: Value,
    },
    /// Secondary-index equality lookup.
    IndexLookup {
        /// The indexed field.
        field: String,
        /// The equality value.
        value: Value,
    },
    /// Document-path index equality lookup.
    DocumentPath {
        /// The dotted document path.
        path: String,
        /// The equality value.
        value: Value,
    },
    /// Full-text index lookup (unranked boolean `contains_text`).
    FullText {
        /// The text field.
        field: String,
        /// The query text.
        query: String,
    },
    /// Ranked full-text (BM25) search.
    TextRanked {
        /// The text field.
        field: String,
    },
    /// Exact vector nearest-neighbour search.
    Vector {
        /// The vector field.
        field: String,
        /// Requested neighbour count.
        k: usize,
        /// The metric name.
        metric: String,
    },
    /// Hybrid text-plus-vector search.
    Hybrid {
        /// The text field.
        text_field: String,
        /// The vector field.
        vector_field: String,
    },
    /// Full collection scan.
    Scan,
}

impl Access {
    /// The index name this access uses to seed selection, if any.
    pub fn used_index(&self) -> Option<String> {
        match self {
            Access::PointLookup { field, .. }
            | Access::IndexLookup { field, .. }
            | Access::FullText { field, .. }
            | Access::TextRanked { field, .. }
            | Access::Vector { field, .. } => Some(field.clone()),
            Access::DocumentPath { path, .. } => Some(path.clone()),
            Access::Hybrid {
                text_field,
                vector_field,
            } => Some(format!("{text_field}+{vector_field}")),
            Access::Scan => None,
        }
    }
}

/// A node in the serializable plan tree returned by `EXPLAIN`. Leaf nodes are
/// access paths; interior nodes are pipeline operators wrapping their input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum PlanNode {
    /// Primary-key / unique equality lookup (≤ 1 row).
    PointLookup {
        /// The seeding index (field name).
        index: String,
        /// Estimated rows produced.
        estimated_rows: usize,
    },
    /// Secondary-index equality lookup.
    IndexLookup {
        /// The seeding index (field name).
        index: String,
        /// The looked-up field.
        field: String,
        /// Estimated rows produced.
        estimated_rows: usize,
    },
    /// Document-path index lookup.
    DocumentPathIndexLookup {
        /// The seeding index (path).
        index: String,
        /// The dotted document path.
        path: String,
        /// Estimated rows produced.
        estimated_rows: usize,
    },
    /// Full-text index lookup (unranked boolean `contains_text`).
    FullTextIndexLookup {
        /// The seeding index (field).
        index: String,
        /// The text field.
        field: String,
        /// Estimated rows produced.
        estimated_rows: usize,
    },
    /// Ranked full-text (BM25) search seeded by a full-text index.
    FullTextBm25Search {
        /// The seeding index (field).
        index: String,
        /// The text field.
        field: String,
        /// The ranking mode (`bm25` or `term_frequency`).
        rank: String,
        /// The term operator (`or` or `and`).
        operator: String,
        /// Estimated rows produced.
        estimated_rows: usize,
    },
    /// Exact vector nearest-neighbour search.
    VectorSearch {
        /// The vector field.
        field: String,
        /// Requested neighbour count.
        k: usize,
        /// The metric name.
        metric: String,
        /// Estimated rows produced.
        estimated_rows: usize,
    },
    /// Hybrid text-plus-vector search fusing BM25 and exact vector signals.
    HybridSearch {
        /// The text field.
        text_field: String,
        /// The vector field.
        vector_field: String,
        /// The fusion mode (`weighted_sum` or `reciprocal_rank_fusion`).
        fusion: String,
        /// The text candidate source (always the BM25 full-text index).
        text_source: String,
        /// The vector candidate source (always the exact vector index).
        vector_source: String,
        /// Estimated rows produced.
        estimated_rows: usize,
    },
    /// Full collection scan.
    Scan {
        /// The scanned collection.
        collection: String,
        /// Estimated rows produced.
        estimated_rows: usize,
    },
    /// Residual filter applied to the input.
    Filter {
        /// The input node.
        input: Box<PlanNode>,
        /// Estimated rows surviving the filter.
        estimated_rows: usize,
    },
    /// Sort the input by ordering keys.
    Sort {
        /// The input node.
        input: Box<PlanNode>,
        /// Ordering keys.
        keys: Vec<OrderKey>,
    },
    /// Skip leading rows.
    Offset {
        /// The input node.
        input: Box<PlanNode>,
        /// Number of rows skipped.
        offset: usize,
    },
    /// Cap the number of rows.
    Limit {
        /// The input node.
        input: Box<PlanNode>,
        /// Maximum rows.
        limit: usize,
    },
    /// Project a subset of fields.
    Projection {
        /// The input node.
        input: Box<PlanNode>,
        /// Retained fields.
        fields: Vec<String>,
    },
    /// Hydrate related records into each row.
    RelationshipInclude {
        /// The input node.
        input: Box<PlanNode>,
        /// Relationship names hydrated.
        relationships: Vec<String>,
    },
    /// Cursor over the input (paged result).
    Cursor {
        /// The input node.
        input: Box<PlanNode>,
    },
    /// Count rows of the input.
    Count {
        /// The input node.
        input: Box<PlanNode>,
    },
    /// Existence test over the input.
    Exists {
        /// The input node.
        input: Box<PlanNode>,
    },
    /// A mutation (write) operation.
    Mutation {
        /// The mutation kind (insert/update/delete/upsert/bulk_insert).
        kind: String,
        /// The target collection.
        collection: String,
    },
}

/// A complete query plan: the chosen access path, cost/row estimates, and the
/// plan tree.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The chosen access path (with execution data).
    pub access: Access,
    /// The index the access seeds from, if any.
    pub used_index: Option<String>,
    /// Estimated rows produced by the access path.
    pub estimated_rows: usize,
    /// Estimated cost of the chosen plan (lower is better).
    pub estimated_cost: f64,
    /// The serializable plan tree for `EXPLAIN`.
    pub node: PlanNode,
}
