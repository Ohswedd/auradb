//! The Query Intermediate Representation (IR).
//!
//! This is the Aura-Connector-compatible query model. It is intentionally a
//! transparent JSON shape (see `docs/QUERY_ENGINE.md`); a follow-up task pins
//! golden IR fixtures from the real connector. Reads use [`FindQuery`] /
//! [`CountQuery`] / [`ExistsQuery`]; writes use [`Mutation`].

use auradb_core::{Document, Value};
use serde::{Deserialize, Serialize};

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Lte,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Gte,
    /// Membership: the field value equals one of an array of values.
    In,
}

/// A filter predicate tree. Field names may be dotted document paths
/// (e.g. `metadata.status`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Filter {
    /// Logical conjunction.
    And {
        /// Sub-filters that must all match.
        filters: Vec<Filter>,
    },
    /// Logical disjunction.
    Or {
        /// Sub-filters of which at least one must match.
        filters: Vec<Filter>,
    },
    /// Logical negation.
    Not {
        /// The negated sub-filter.
        filter: Box<Filter>,
    },
    /// A field comparison.
    Compare {
        /// The field (dotted path supported).
        field: String,
        /// The comparison operator.
        op: CompareOp,
        /// The right-hand value.
        value: Value,
    },
    /// Case-sensitive substring containment on a string field.
    Contains {
        /// The field (dotted path supported).
        field: String,
        /// The substring to find.
        substring: String,
    },
    /// The field exists and is non-null.
    Exists {
        /// The field (dotted path supported).
        field: String,
    },
    /// Full-text match: the field's tokens contain all tokens of `query`.
    /// Uses a full-text index when one exists, otherwise a tokenized scan.
    ContainsText {
        /// The text field (dotted path supported).
        field: String,
        /// The query text; all of its distinct tokens must be present.
        query: String,
    },
}

/// An ordering key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderKey {
    /// The field to order by (dotted path supported).
    pub field: String,
    /// Descending if true, ascending otherwise.
    #[serde(default)]
    pub desc: bool,
}

/// An exact vector nearest-neighbour clause.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorSearch {
    /// The vector field to search.
    pub field: String,
    /// The query vector.
    pub query: Vec<f32>,
    /// The number of nearest neighbours to consider.
    pub k: usize,
    /// The metric name (`cosine`, `euclidean`, `dot_product`).
    pub metric: String,
}

/// The default BM25 term-saturation parameter `k1`.
pub const BM25_DEFAULT_K1: f32 = 1.2;
/// The default BM25 length-normalization parameter `b`.
pub const BM25_DEFAULT_B: f32 = 0.75;

/// How a ranked full-text query combines its terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextOperator {
    /// A document matches if it contains any query term; all matching terms
    /// contribute to its relevance score. This is the relevance-ranking default.
    #[default]
    Or,
    /// A document matches only if it contains every distinct query term.
    And,
}

impl TextOperator {
    /// Whether every query term is required (AND semantics).
    pub fn require_all(self) -> bool {
        matches!(self, TextOperator::And)
    }
}

/// The ranking function applied to a ranked full-text query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextRank {
    /// Okapi BM25 relevance ranking (the default for ranked text search).
    #[default]
    Bm25,
    /// Summed term-frequency ranking, matching legacy `contains_text` scoring.
    TermFrequency,
}

/// A ranked full-text search clause (BM25-style relevance). Distinct from the
/// `contains_text` filter, which is an unranked boolean-AND predicate preserved
/// for compatibility; this clause returns documents ordered by relevance score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextSearch {
    /// The full-text indexed field (dotted path supported).
    pub field: String,
    /// The query text.
    pub query: String,
    /// How query terms are combined.
    #[serde(default)]
    pub operator: TextOperator,
    /// The ranking function.
    #[serde(default)]
    pub rank: TextRank,
    /// BM25 `k1` override (defaults to [`BM25_DEFAULT_K1`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k1: Option<f32>,
    /// BM25 `b` override (defaults to [`BM25_DEFAULT_B`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<f32>,
}

/// The score-fusion strategy for a hybrid query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FusionMode {
    /// Min-max normalize each signal's scores to `[0, 1]`, then combine with the
    /// configured weights.
    #[default]
    WeightedSum,
    /// Reciprocal rank fusion: combine `weight / (rrf_k + rank)` over each
    /// signal's rank ordering. Robust to score-scale differences.
    ReciprocalRankFusion,
}

/// Per-signal weights for hybrid fusion. Both default to `0.5`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HybridWeights {
    /// Weight applied to the text relevance signal.
    pub text: f32,
    /// Weight applied to the vector similarity signal.
    pub vector: f32,
}

impl Default for HybridWeights {
    fn default() -> Self {
        HybridWeights {
            text: 0.5,
            vector: 0.5,
        }
    }
}

/// A hybrid text-plus-vector search clause combining BM25 text relevance with
/// exact vector similarity under a deterministic fusion of the two signals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HybridSearch {
    /// The full-text indexed field.
    pub text_field: String,
    /// The text query.
    pub text_query: String,
    /// The vector field.
    pub vector_field: String,
    /// The query vector.
    pub vector: Vec<f32>,
    /// The number of fused results to return.
    pub top_k: usize,
    /// The vector metric name (`cosine`, `euclidean`, `dot_product`); defaults to
    /// `cosine` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    /// Per-signal fusion weights.
    #[serde(default)]
    pub weights: HybridWeights,
    /// The fusion strategy.
    #[serde(default)]
    pub fusion: FusionMode,
    /// How the text terms are combined.
    #[serde(default)]
    pub operator: TextOperator,
    /// BM25 `k1` override (defaults to [`BM25_DEFAULT_K1`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k1: Option<f32>,
    /// BM25 `b` override (defaults to [`BM25_DEFAULT_B`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<f32>,
}

impl HybridSearch {
    /// The vector metric, defaulting to `cosine`.
    pub fn metric_name(&self) -> &str {
        self.metric.as_deref().unwrap_or("cosine")
    }
}

/// A read query returning matching rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindQuery {
    /// The collection to query.
    pub collection: String,
    /// Optional filter predicate.
    #[serde(default)]
    pub filter: Option<Filter>,
    /// Optional vector nearest-neighbour clause (applied before ordering).
    #[serde(default)]
    pub vector: Option<VectorSearch>,
    /// Optional ranked full-text (BM25) clause. Mutually exclusive with `vector`
    /// and `hybrid`; orders results by relevance score. Boxed to keep the
    /// `ReadRequest` enum compact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_search: Option<Box<TextSearch>>,
    /// Optional hybrid text-plus-vector clause. Mutually exclusive with `vector`
    /// and `text_search`; orders results by fused score. Boxed to keep the
    /// `ReadRequest` enum compact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hybrid: Option<Box<HybridSearch>>,
    /// Ordering keys (ignored when a vector, text-search, or hybrid clause is
    /// present, which order by score).
    #[serde(default)]
    pub order_by: Vec<OrderKey>,
    /// Maximum number of rows to return.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Number of leading rows to skip.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Optional projection: only these fields are returned.
    #[serde(default)]
    pub projection: Option<Vec<String>>,
    /// Relationship field names to hydrate into each row.
    #[serde(default)]
    pub includes: Vec<String>,
}

impl FindQuery {
    /// Construct a minimal find over a collection.
    pub fn new(collection: impl Into<String>) -> Self {
        FindQuery {
            collection: collection.into(),
            filter: None,
            vector: None,
            text_search: None,
            hybrid: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            projection: None,
            includes: Vec::new(),
        }
    }

    /// Validate that at most one ranked-retrieval clause is set. Returns a
    /// structured error otherwise so the engine rejects ambiguous requests.
    pub fn validate_search_clauses(&self) -> Result<(), String> {
        let set = [
            self.vector.is_some(),
            self.text_search.is_some(),
            self.hybrid.is_some(),
        ]
        .into_iter()
        .filter(|x| *x)
        .count();
        if set > 1 {
            return Err(
                "at most one of `vector`, `text_search`, or `hybrid` may be set on a query".into(),
            );
        }
        Ok(())
    }
}

/// A count query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CountQuery {
    /// The collection.
    pub collection: String,
    /// Optional filter.
    #[serde(default)]
    pub filter: Option<Filter>,
}

/// An existence query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExistsQuery {
    /// The collection.
    pub collection: String,
    /// Optional filter.
    #[serde(default)]
    pub filter: Option<Filter>,
}

/// A read request opcode payload (`Opcode::Query`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "query", rename_all = "snake_case")]
pub enum ReadRequest {
    /// Find matching rows.
    Find(FindQuery),
    /// Count matching rows.
    Count(CountQuery),
    /// Test whether any row matches.
    Exists(ExistsQuery),
}

/// A mutation request payload (`Opcode::Mutate`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mutation", rename_all = "snake_case")]
pub enum Mutation {
    /// Insert a single record. Fails if the primary key already exists.
    Insert {
        /// Target collection.
        collection: String,
        /// The record fields (must include the primary key).
        fields: Document,
    },
    /// Insert many records atomically.
    BulkInsert {
        /// Target collection.
        collection: String,
        /// The records' fields.
        records: Vec<Document>,
    },
    /// Update fields of all records matching a filter.
    Update {
        /// Target collection.
        collection: String,
        /// Which records to update.
        filter: Option<Filter>,
        /// Field assignments to merge into matched records.
        set: Document,
    },
    /// Delete all records matching a filter.
    Delete {
        /// Target collection.
        collection: String,
        /// Which records to delete.
        filter: Option<Filter>,
    },
    /// Insert a record, or replace it if the primary key already exists.
    Upsert {
        /// Target collection.
        collection: String,
        /// The record fields (must include the primary key).
        fields: Document,
    },
}

impl Mutation {
    /// The collection this mutation targets.
    pub fn collection(&self) -> &str {
        match self {
            Mutation::Insert { collection, .. }
            | Mutation::BulkInsert { collection, .. }
            | Mutation::Update { collection, .. }
            | Mutation::Delete { collection, .. }
            | Mutation::Upsert { collection, .. } => collection,
        }
    }
}

/// The result of a mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutationResult {
    /// Number of records inserted.
    #[serde(default)]
    pub inserted: usize,
    /// Number of records updated.
    #[serde(default)]
    pub updated: usize,
    /// Number of records deleted.
    #[serde(default)]
    pub deleted: usize,
    /// The ids affected (as hex strings).
    #[serde(default)]
    pub ids: Vec<String>,
}

impl MutationResult {
    /// An empty result.
    pub fn empty() -> Self {
        MutationResult {
            inserted: 0,
            updated: 0,
            deleted: 0,
            ids: Vec::new(),
        }
    }
}

/// One result row from a [`FindQuery`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// The record id (hex string).
    pub id: String,
    /// The (possibly projected) fields.
    pub fields: Document,
    /// Relevance/similarity score, when the query had a vector, text-search, or
    /// hybrid clause. For hybrid this is the fused score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    /// The text-relevance component of a hybrid score, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_score: Option<f32>,
    /// The vector-similarity component of a hybrid score, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_score: Option<f32>,
    /// The 1-based rank of this row within a ranked result set, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<usize>,
    /// Hydrated related records keyed by relationship name.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub includes: std::collections::BTreeMap<String, Vec<Document>>,
}

/// A page of query results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResultPage {
    /// The rows in this page.
    pub rows: Vec<Row>,
    /// A cursor id to fetch the next page, if more rows remain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_id: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_query_roundtrips() {
        let q = FindQuery {
            collection: "Doc".into(),
            filter: Some(Filter::And {
                filters: vec![
                    Filter::Compare {
                        field: "status".into(),
                        op: CompareOp::Eq,
                        value: Value::Text("published".into()),
                    },
                    Filter::Exists {
                        field: "metadata.source".into(),
                    },
                ],
            }),
            vector: None,
            text_search: None,
            hybrid: None,
            order_by: vec![OrderKey {
                field: "created_at".into(),
                desc: true,
            }],
            limit: Some(10),
            offset: Some(5),
            projection: Some(vec!["id".into(), "title".into()]),
            includes: vec!["owner".into()],
        };
        let json = serde_json::to_string(&q).unwrap();
        let back: FindQuery = serde_json::from_str(&json).unwrap();
        assert_eq!(q, back);
    }

    #[test]
    fn text_search_roundtrips_with_defaults() {
        let mut q = FindQuery::new("Doc");
        q.text_search = Some(Box::new(TextSearch {
            field: "body".into(),
            query: "raft consensus".into(),
            operator: TextOperator::default(),
            rank: TextRank::default(),
            k1: None,
            b: None,
        }));
        let json = serde_json::to_string(&q).unwrap();
        let back: FindQuery = serde_json::from_str(&json).unwrap();
        assert_eq!(q, back);
        assert!(q.validate_search_clauses().is_ok());
    }

    #[test]
    fn hybrid_roundtrips_and_defaults_apply() {
        let h = HybridSearch {
            text_field: "body".into(),
            text_query: "vector index".into(),
            vector_field: "embedding".into(),
            vector: vec![0.1, 0.2, 0.3],
            top_k: 5,
            metric: None,
            weights: HybridWeights::default(),
            fusion: FusionMode::default(),
            operator: TextOperator::default(),
            k1: None,
            b: None,
        };
        let json = serde_json::to_string(&h).unwrap();
        let back: HybridSearch = serde_json::from_str(&json).unwrap();
        assert_eq!(h, back);
        assert_eq!(back.metric_name(), "cosine");
        assert_eq!(back.weights.text, 0.5);
    }

    #[test]
    fn conflicting_search_clauses_rejected() {
        let mut q = FindQuery::new("Doc");
        q.vector = Some(VectorSearch {
            field: "embedding".into(),
            query: vec![1.0],
            k: 1,
            metric: "cosine".into(),
        });
        q.text_search = Some(Box::new(TextSearch {
            field: "body".into(),
            query: "x".into(),
            operator: TextOperator::default(),
            rank: TextRank::default(),
            k1: None,
            b: None,
        }));
        assert!(q.validate_search_clauses().is_err());
    }

    #[test]
    fn legacy_find_query_json_without_search_fields_deserializes() {
        // A request from an older connector that omits text_search/hybrid.
        let json = r#"{"collection":"Doc"}"#;
        let q: FindQuery = serde_json::from_str(json).unwrap();
        assert!(q.text_search.is_none());
        assert!(q.hybrid.is_none());
    }

    #[test]
    fn read_request_tagged() {
        let r = ReadRequest::Count(CountQuery {
            collection: "C".into(),
            filter: None,
        });
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["query"], "count");
    }

    #[test]
    fn mutation_roundtrips() {
        let mut fields = Document::new();
        fields.insert("id".into(), Value::Text("x".into()));
        let m = Mutation::Upsert {
            collection: "C".into(),
            fields,
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: Mutation = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
        assert_eq!(back.collection(), "C");
    }
}
