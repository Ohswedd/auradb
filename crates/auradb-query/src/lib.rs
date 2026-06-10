//! # auradb-query
//!
//! The AuraDB Query IR and its executor. [`ir`] defines the
//! Aura-Connector-compatible query/mutation model; [`eval`] evaluates filters;
//! [`exec`] selects candidates (using indexes when possible), filters, orders,
//! projects, hydrates relationships, and produces EXPLAIN plans over a
//! [`exec::DataSource`]; [`migrate`] estimates schema migration impact.
//!
//! Read execution lives here; mutation *application* (versioning, index updates,
//! durability) is performed by the `auradb` engine, which reuses [`eval`] to
//! select records for update/delete.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod aggregate;
pub mod cursor;
pub mod eval;
pub mod exec;
pub mod ir;
pub mod migrate;
pub mod plan;
pub mod planner;
pub mod relevance;
pub mod stats;

pub use aggregate::{
    execute_aggregate, AggregateResult, FacetBucket, FacetValues, GroupBucket, GroupByResult,
    MetricValue,
};
pub use cursor::{paginate_ranked, RankedPage};
pub use exec::{
    execute_count, execute_exists, execute_find, execute_find_within, explain, explain_analyze,
    materialize, materialize_page, DataSource, Deadline, ExplainAnalysis, ExplainPlan, HybridPlan,
    PlannedFind, Scored, Strategy, TextSearchPlan, VectorPlan,
};
pub use ir::{
    AggregateMetric, AggregateOp, AggregateQuery, AnnFallback, AnnParams, CompareOp, CountQuery,
    ExistsQuery, ANN_PREVIEW_MIN_VECTORS,
};
pub use ir::{
    FacetRequest, Filter, FindQuery, FusionMode, HybridSearch, HybridWeights, Mutation,
    MutationResult, OrderKey, QueryResultPage, RankedPageResult, ReadRequest, Row,
    SearchPageRequest, TextOperator, TextRank, TextSearch, VectorSearch, BM25_DEFAULT_B,
    BM25_DEFAULT_K1, DEFAULT_FACET_LIMIT, DEFAULT_GROUP_LIMIT,
};
pub use migrate::{estimate as estimate_migration, MigrationEstimate};
pub use plan::{Access, Plan, PlanNode};
pub use relevance::{
    dcg_at_k, mrr_at_k, ndcg_at_k, recall_at_k, relevant_set, RELEVANT_GRADE_THRESHOLD,
};
pub use stats::{CollectionStats, PlannerStats, STATS_FORMAT_VERSION};
