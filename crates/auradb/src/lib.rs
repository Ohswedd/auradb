//! # auradb
//!
//! The embeddable AuraDB engine. [`Engine`] composes the storage engine,
//! indexes, transactions, and query execution behind one synchronous,
//! thread-safe, cheaply-cloneable handle. The server crate wraps it for the
//! network; the CLI and benchmarks use it directly.
//!
//! ```no_run
//! use auradb::Engine;
//! use auradb::query::{FindQuery, Mutation};
//! use auradb::core::{CollectionSchema, FieldDef, FieldType, Document, Value};
//!
//! let engine = Engine::open("/tmp/auradb-example").unwrap();
//! engine
//!     .create_schema(CollectionSchema::new("User").with_field(FieldDef {
//!         name: "id".into(),
//!         field_type: FieldType::Uuid,
//!         primary_key: true,
//!         unique: true,
//!         nullable: false,
//!         indexed: false,
//!     }))
//!     .unwrap();
//! let mut fields = Document::new();
//! fields.insert("id".into(), Value::Text("u1".into()));
//! engine.insert("User", fields).unwrap();
//! let rows = engine.find(&FindQuery::new("User")).unwrap();
//! assert_eq!(rows.len(), 1);
//! # let _ = Mutation::Delete { collection: "User".into(), filter: None };
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod clock;
mod engine;
mod idgen;

pub use clock::WallClock;
pub use engine::{
    ActiveTransaction, Engine, EngineOptions, EngineStats, IndexLoadReport, ReplicatedLog,
    SearchIndexInfo, TextIndexInfo, TxnState, VectorIndexInfo,
};

/// Re-export of core types.
pub mod core {
    pub use auradb_core::*;
}

/// Re-export of the query IR and result types.
pub mod query {
    pub use auradb_query::*;
}

/// Re-export of storage types (options, compaction and GC reports, and the
/// on-disk manifest/catalog primitives consulted by consistency checks).
pub mod storage {
    pub use auradb_storage::{
        Catalog, CompactionReport, GcReport, Manifest, StorageOptions, FORMAT_VERSION,
        MIN_READABLE_FORMAT_VERSION,
    };
}

pub use auradb_txn::Transaction;
