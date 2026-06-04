//! # auradb-core
//!
//! Foundational types shared across every AuraDB crate: errors and stable error
//! codes, logical identifiers, the [`Value`] data model, schema types, the
//! [`Record`] type, capability advertisement, and a logical clock.
//!
//! This crate has no dependencies on other AuraDB crates and defines the
//! contracts everything else builds on.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod capability;
pub mod clock;
pub mod error;
pub mod ids;
pub mod record;
pub mod schema;
pub mod value;

pub use capability::{Capability, ServerCapabilities};
pub use clock::LogicalClock;
pub use error::{Error, ErrorCode, Result};
pub use ids::{CollectionId, RecordId, SchemaId, TxnId};
pub use record::Record;
pub use schema::{Cardinality, CollectionSchema, FieldDef, FieldType, OnDelete, Relationship};
pub use value::{Document, Value};
