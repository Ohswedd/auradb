//! The Query Intermediate Representation (IR).
//!
//! This is the Aura-Connector-compatible query model. It is intentionally a
//! transparent JSON shape (see `docs/QUERY_ENGINE.md`); a follow-up task pins
//! golden IR fixtures from the real connector. Reads use [`FindQuery`] /
//! [`CountQuery`] / [`ExistsQuery`]; writes use [`Mutation`].

use auradb_core::{Document, Value};
use auradb_index::{Analyzer, AnalyzerPreset};
use serde::{Deserialize, Serialize};

use crate::snippet::Snippet;

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

/// The minimum number of indexed vectors a field needs for the approximate
/// (HNSW) preview to be meaningful. Below this, the navigable graph degenerates
/// toward full connectivity and offers no benefit over the exact scan (which is
/// also cheaper and is the correctness baseline), so the preview is treated as
/// unavailable and the [`AnnParams::fallback`] policy applies.
pub const ANN_PREVIEW_MIN_VECTORS: usize = 16;

/// What to do when the opt-in HNSW preview is unavailable for a query (for
/// example, the field has fewer than [`ANN_PREVIEW_MIN_VECTORS`] vectors). Exact
/// search is always available and always correct, so the default is to fall back
/// to it; callers that specifically require approximate semantics can ask for a
/// structured error instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnnFallback {
    /// Use exact search when the preview is unavailable (the default). The result
    /// is the exact top-k; the plan reports `exact_fallback = true`.
    #[default]
    Exact,
    /// Return a structured error when the preview is unavailable rather than
    /// silently using exact search.
    Error,
}

/// Opt-in approximate (HNSW) vector-search **preview** parameters. Present on a
/// query only when it explicitly requests approximate search; absent means exact
/// search — the default and the correctness baseline. All fields are optional and
/// fall back to engine defaults.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct AnnParams {
    /// HNSW graph degree `M` (graph build parameter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub m: Option<usize>,
    /// HNSW `efConstruction` (graph build beam width).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ef_construction: Option<usize>,
    /// HNSW `efSearch` (query beam width; higher = more recall, more cost).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ef_search: Option<usize>,
    /// What to do when the preview is unavailable for this query. Additive:
    /// older connectors omit it and get the default (`exact`).
    #[serde(default)]
    pub fallback: AnnFallback,
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
    /// Optional query-time analyzer preset (`default`, `simple`, `ascii_fold`,
    /// `keyword`, `english_basic`). Absent or `default` preserves v1.x behavior
    /// exactly. Additive and defaulted so older connectors are unaffected; the
    /// server validates the name and applies it symmetrically to the query and the
    /// index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyzer: Option<String>,
}

impl TextSearch {
    /// The resolved query-time analyzer (defaults to [`AnalyzerPreset::Default`]).
    /// Returns a structured error for an unknown analyzer name.
    pub fn resolved_analyzer(&self) -> Result<Analyzer, String> {
        resolve_analyzer(self.analyzer.as_deref())
    }
}

/// Parse an optional analyzer name into an [`Analyzer`], defaulting to
/// [`AnalyzerPreset::Default`] when absent or `"default"`. The error is a plain
/// message so callers can wrap it in their own error type.
pub(crate) fn resolve_analyzer(name: Option<&str>) -> Result<Analyzer, String> {
    match name {
        None | Some("default") | Some("") => Ok(Analyzer::new(AnalyzerPreset::Default)),
        Some(other) => Analyzer::parse(other).map_err(|e| e.to_string()),
    }
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
    /// Optional query-time analyzer preset applied to the text signal (see
    /// [`TextSearch::analyzer`]). Additive and defaulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyzer: Option<String>,
}

impl HybridSearch {
    /// The vector metric, defaulting to `cosine`.
    pub fn metric_name(&self) -> &str {
        self.metric.as_deref().unwrap_or("cosine")
    }

    /// The resolved query-time analyzer for the text signal (defaults to
    /// [`AnalyzerPreset::Default`]). Returns a structured error for an unknown name.
    pub fn resolved_analyzer(&self) -> Result<Analyzer, String> {
        resolve_analyzer(self.analyzer.as_deref())
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
    /// Opt-in approximate (HNSW) vector-search preview. When present **and** a
    /// `vector` clause is set, the engine uses the approximate index for that
    /// clause; absent means exact search (the default). Additive and defaulted so
    /// older connectors are unaffected, and exact remains the correctness baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_ann: Option<AnnParams>,
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
    /// Optional per-query execution deadline in milliseconds. When set (and
    /// non-zero), execution is cooperatively cancelled with a structured
    /// `query_timeout` error once the budget is exceeded. The server clamps this
    /// against its configured maximum; omitting it falls back to that maximum.
    /// Additive and defaulted so older connectors that omit it are unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Opt-in request for search snippets/highlights on a ranked text (or hybrid)
    /// query. Absent means no snippets are produced (existing behavior). Additive
    /// and defaulted so older connectors are unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<SnippetRequest>,
}

/// An opt-in request for plain-text search snippets/highlights, attached to a
/// [`FindQuery`] alongside a `text_search` or `hybrid` clause.
///
/// Snippets are only ever produced for the stored text fields named in
/// [`fields`](SnippetRequest::fields) (the allowlist) — a field absent from the
/// list is never read, so internal or unrequested fields cannot leak. Fragment
/// count and length are server-clamped. Snippet text is plain text; the highlight
/// ranges are byte offsets into the fragment text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetRequest {
    /// The stored text fields eligible for snippets (the allowlist). A field that
    /// is absent, non-textual, or internal is skipped, never returned.
    pub fields: Vec<String>,
    /// Maximum fragments per field. Defaulted and clamped by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fragments: Option<usize>,
    /// Maximum characters per fragment. Defaulted and clamped by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fragment_chars: Option<usize>,
}

impl FindQuery {
    /// Construct a minimal find over a collection.
    pub fn new(collection: impl Into<String>) -> Self {
        FindQuery {
            collection: collection.into(),
            filter: None,
            vector: None,
            vector_ann: None,
            text_search: None,
            hybrid: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            projection: None,
            includes: Vec::new(),
            timeout_ms: None,
            snippet: None,
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

/// An aggregation metric operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateOp {
    /// Count of matched records (ignores `field`).
    Count,
    /// Minimum of a numeric/orderable field over the matched set.
    Min,
    /// Maximum of a numeric/orderable field over the matched set.
    Max,
    /// Arithmetic mean of a numeric field over the matched set. Only `Int`/`Float`
    /// values contribute; null, missing, and non-numeric values are skipped. The
    /// result is a `Float`, or null when the matched set carried no numeric value.
    Avg,
}

impl AggregateOp {
    /// Whether this operator requires a `field`.
    pub fn needs_field(self) -> bool {
        matches!(self, AggregateOp::Min | AggregateOp::Max | AggregateOp::Avg)
    }

    /// The stable string name (`count`, `min`, `max`, `avg`).
    pub fn name(self) -> &'static str {
        match self {
            AggregateOp::Count => "count",
            AggregateOp::Min => "min",
            AggregateOp::Max => "max",
            AggregateOp::Avg => "avg",
        }
    }
}

/// A single aggregation metric: an operator over an optional field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateMetric {
    /// The aggregation operator.
    pub op: AggregateOp,
    /// The target field (required for `min`/`max`, ignored for `count`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

/// The default number of facet buckets returned when a facet omits `limit`.
pub const DEFAULT_FACET_LIMIT: usize = 10;

/// The default cap on returned groups when a grouped aggregate omits
/// `group_limit`. Groups are ordered by descending count, then ascending key, so
/// the truncation is deterministic; the full distinct-group count is reported
/// separately so a truncated result is never silently mistaken for complete.
pub const DEFAULT_GROUP_LIMIT: usize = 1000;

/// A request for a terms facet over a scalar field: the distinct values of the
/// field within the matched set, with per-value counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FacetRequest {
    /// The scalar field (dotted document path supported).
    pub field: String,
    /// The maximum number of buckets to return (defaults to
    /// [`DEFAULT_FACET_LIMIT`]). Buckets are ordered by descending count, then
    /// ascending value, so the truncation is deterministic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

impl FacetRequest {
    /// The effective bucket limit.
    pub fn effective_limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_FACET_LIMIT)
    }
}

/// An aggregation/faceting query over a collection. Aggregations and facets are
/// computed over the same matched set (filter, or an optional ranked-text
/// candidate set for search facets). Additive to the read surface: older
/// connectors never send it, and a server that predates it rejects the unknown
/// `query` tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateQuery {
    /// The collection to aggregate over.
    pub collection: String,
    /// Optional filter applied before aggregation.
    #[serde(default)]
    pub filter: Option<Filter>,
    /// Optional ranked full-text clause: when present, facets and metrics are
    /// computed over the BM25 candidate set (a "search facet"). The filter, if
    /// any, is still applied as a residual.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_search: Option<Box<TextSearch>>,
    /// Terms facets to compute.
    #[serde(default)]
    pub facets: Vec<FacetRequest>,
    /// Aggregation metrics to compute.
    #[serde(default)]
    pub metrics: Vec<AggregateMetric>,
    /// Optional GROUP BY over a single scalar field. When set, the result carries
    /// a `groups` list: one bucket per distinct value of this field within the
    /// matched set (after the residual filter and/or BM25 candidate scoping),
    /// each carrying the requested `metrics` recomputed over that group's records.
    /// Records whose group field is null or missing are excluded from grouping.
    /// Additive: older connectors never send it; older servers reject the unknown
    /// `query` tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<String>,
    /// Optional cap on the number of returned groups (defaults to
    /// [`DEFAULT_GROUP_LIMIT`]). Groups are ordered by descending count then
    /// ascending key before truncation. Ignored unless `group_by` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_limit: Option<usize>,
    /// Optional per-query execution deadline in milliseconds (see
    /// [`FindQuery::timeout_ms`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl AggregateQuery {
    /// Construct a minimal aggregate over a collection.
    pub fn new(collection: impl Into<String>) -> Self {
        AggregateQuery {
            collection: collection.into(),
            filter: None,
            text_search: None,
            facets: Vec::new(),
            metrics: Vec::new(),
            group_by: None,
            group_limit: None,
            timeout_ms: None,
        }
    }

    /// The effective group cap (defaults to [`DEFAULT_GROUP_LIMIT`]).
    pub fn effective_group_limit(&self) -> usize {
        self.group_limit.unwrap_or(DEFAULT_GROUP_LIMIT)
    }

    /// Validate metric/facet shapes: `min`/`max` require a field, `count` must
    /// not carry one, and facet/metric fields must be non-empty.
    pub fn validate(&self) -> Result<(), String> {
        if self.facets.is_empty() && self.metrics.is_empty() && self.group_by.is_none() {
            return Err(
                "an aggregate query must request at least one facet, metric, or group_by".into(),
            );
        }
        if let Some(field) = &self.group_by {
            if field.is_empty() {
                return Err("group_by field must not be empty".into());
            }
        }
        if let Some(limit) = self.group_limit {
            if limit == 0 {
                return Err("group_limit must be >= 1".into());
            }
            if self.group_by.is_none() {
                return Err("group_limit requires group_by".into());
            }
        }
        for m in &self.metrics {
            match (m.op.needs_field(), &m.field) {
                (true, None) => {
                    return Err(format!("aggregation `{}` requires a field", m.op.name()))
                }
                (true, Some(f)) | (false, Some(f)) if f.is_empty() => {
                    return Err("aggregation field must not be empty".into());
                }
                _ => {}
            }
        }
        for f in &self.facets {
            if f.field.is_empty() {
                return Err("facet field must not be empty".into());
            }
        }
        Ok(())
    }
}

/// A ranked-pagination request: page a ranked search (`vector` / `text_search` /
/// `hybrid`) by stable opaque cursor token. Additive to the read surface; older
/// servers reject the unknown `query` tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchPageRequest {
    /// The ranked query. Must carry exactly one ranked clause (`vector`,
    /// `text_search`, or `hybrid`); its own `limit`/`offset` are ignored for
    /// cursor paging, while a `hybrid` `top_k` or `vector` `k` still bounds the
    /// result set.
    pub find: FindQuery,
    /// Maximum rows to return for this page (>= 1).
    pub page_size: usize,
    /// The opaque cursor token returned by a previous page, or `None` for the
    /// first page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A page of ranked rows plus the token to fetch the next page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedPageResult {
    /// The ranked rows in this page (with stable cross-page `rank`).
    pub rows: Vec<Row>,
    /// The opaque token for the next page, or `None` when this is the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
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
    /// Compute aggregations and/or terms facets over a collection.
    Aggregate(AggregateQuery),
    /// Page a ranked search by stable cursor token.
    SearchPage(SearchPageRequest),
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
    /// Opt-in plain-text snippets/highlights, one per snippet-eligible field, when
    /// the query carried a [`SnippetRequest`]. Empty (and omitted from the wire)
    /// otherwise, so existing clients are unaffected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snippets: Vec<Snippet>,
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
            vector_ann: None,
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
            timeout_ms: None,
            snippet: None,
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
            analyzer: None,
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
            analyzer: None,
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
            analyzer: None,
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

    #[test]
    fn serde_missing_analyzer_defaults() {
        // A text_search from an older client omits `analyzer`; it deserializes to
        // None and resolves to the default analyzer.
        let json = r#"{"field":"body","query":"x","operator":"or","rank":"bm25"}"#;
        let ts: TextSearch = serde_json::from_str(json).unwrap();
        assert!(ts.analyzer.is_none());
        assert_eq!(ts.resolved_analyzer().unwrap().name(), "default");
        // A default analyzer is omitted from the serialized form (byte-compatible).
        let out = serde_json::to_value(&ts).unwrap();
        assert!(out.get("analyzer").is_none());
    }

    #[test]
    fn serde_analyzer_roundtrips_when_set() {
        let ts = TextSearch {
            field: "body".into(),
            query: "café".into(),
            operator: TextOperator::Or,
            rank: TextRank::Bm25,
            k1: None,
            b: None,
            analyzer: Some("ascii_fold".into()),
        };
        let json = serde_json::to_string(&ts).unwrap();
        assert!(json.contains("ascii_fold"));
        let back: TextSearch = serde_json::from_str(&json).unwrap();
        assert_eq!(back.analyzer.as_deref(), Some("ascii_fold"));
        assert_eq!(back.resolved_analyzer().unwrap().name(), "ascii_fold");
    }

    #[test]
    fn serde_unknown_extra_fields_compat() {
        // A request from a NEWER client carrying fields this build does not know
        // must still deserialize (forward-compatible), ignoring the unknown field.
        let json = r#"{"collection":"Doc","text_search":{"field":"body","query":"x",
            "analyzer":"simple","future_field":{"nested":true}},"future_top":42}"#;
        let q: FindQuery = serde_json::from_str(json).unwrap();
        assert_eq!(q.collection, "Doc");
        let ts = q.text_search.unwrap();
        assert_eq!(ts.analyzer.as_deref(), Some("simple"));
    }

    #[test]
    fn serde_missing_snippet_request_defaults() {
        // No snippet field -> None (no snippets), and it is omitted on the wire.
        let q = FindQuery::new("Doc");
        assert!(q.snippet.is_none());
        let json = serde_json::to_value(&q).unwrap();
        assert!(json.get("snippet").is_none());
        // A request with a snippet clause round-trips.
        let mut q2 = FindQuery::new("Doc");
        q2.snippet = Some(SnippetRequest {
            fields: vec!["body".into()],
            max_fragments: Some(2),
            fragment_chars: None,
        });
        let back: FindQuery = serde_json::from_str(&serde_json::to_string(&q2).unwrap()).unwrap();
        assert_eq!(back.snippet, q2.snippet);
    }

    #[test]
    fn old_clients_ignore_snippet_response_fields() {
        // A Row carrying snippets serializes them under `snippets`; a Row without
        // snippets omits the field entirely so an older client never sees it.
        let plain = Row {
            id: "1".into(),
            fields: Document::new(),
            score: None,
            text_score: None,
            vector_score: None,
            rank: None,
            includes: Default::default(),
            snippets: Vec::new(),
        };
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("snippets").is_none());
        // And a legacy row JSON (no `snippets` key) still deserializes.
        let legacy = r#"{"id":"1","fields":{}}"#;
        let back: Row = serde_json::from_str(legacy).unwrap();
        assert!(back.snippets.is_empty());
    }
}
