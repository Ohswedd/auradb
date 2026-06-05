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

pub mod eval;
pub mod exec;
pub mod ir;
pub mod migrate;
pub mod plan;
pub mod planner;
pub mod stats;

pub use exec::{
    execute_count, execute_exists, execute_find, explain, explain_analyze, materialize, DataSource,
    ExplainAnalysis, ExplainPlan, PlannedFind, Strategy, VectorPlan,
};
pub use ir::{
    CompareOp, CountQuery, ExistsQuery, Filter, FindQuery, Mutation, MutationResult, OrderKey,
    QueryResultPage, ReadRequest, Row, VectorSearch,
};
pub use migrate::{estimate as estimate_migration, MigrationEstimate};
pub use plan::{Access, Plan, PlanNode};
pub use stats::{CollectionStats, PlannerStats, STATS_FORMAT_VERSION};
