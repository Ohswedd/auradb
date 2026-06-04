//! # auradb-conformance
//!
//! A protocol [`Client`] (the Aura Connector stand-in) and a conformance
//! scenario suite ([`run_all`]) exercising every first-release capability over
//! the wire: connect, ping, health, schema, CRUD, filters, documents, vectors,
//! relationships, cursors/streaming, explain, migration estimate, and
//! transactions.
//!
//! See `docs/CONFORMANCE.md` and `tests/conformance/python/` for the
//! Python-client harness instructions.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod client;
mod scenarios;

pub use client::{Client, ClientTls, ConnectOptions};
pub use scenarios::{run_all, ConformanceReport, ScenarioOutcome};
