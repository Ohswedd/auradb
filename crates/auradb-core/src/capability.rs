//! Server capability advertisement.
//!
//! Capabilities let a client discover what this server actually implements. The
//! contract forbids claiming features that are not real, so unsupported
//! operations return a structured [`crate::error::Error::Unsupported`] referring
//! to a capability name, and the capability set advertised at connect time lists
//! only implemented features.

use serde::{Deserialize, Serialize};

/// A named server capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Persistent append-only storage with recovery.
    PersistentStorage,
    /// Single-node staged transactions with commit/rollback.
    Transactions,
    /// Primary, unique, and secondary indexes.
    SecondaryIndexes,
    /// Nested document fields and path access.
    DocumentFields,
    /// Exact vector nearest-neighbour search.
    VectorExactSearch,
    /// Relationship links and include hydration.
    Relationships,
    /// Server-side cursors with paging.
    ServerCursors,
    /// EXPLAIN query plans.
    Explain,
    /// Migration impact estimation.
    MigrationEstimate,
    /// Metrics and health endpoints.
    Observability,
}

impl Capability {
    /// The full set of capabilities implemented by this single-node release.
    pub fn implemented() -> Vec<Capability> {
        vec![
            Capability::PersistentStorage,
            Capability::Transactions,
            Capability::SecondaryIndexes,
            Capability::DocumentFields,
            Capability::VectorExactSearch,
            Capability::Relationships,
            Capability::ServerCursors,
            Capability::Explain,
            Capability::MigrationEstimate,
            Capability::Observability,
        ]
    }
}

/// The server's advertised capabilities and version, returned at connect time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// The server software version.
    pub server_version: String,
    /// The maximum supported protocol version.
    pub protocol_version: u8,
    /// The list of implemented capabilities.
    pub capabilities: Vec<Capability>,
}

impl ServerCapabilities {
    /// Build the capability set for the current build.
    pub fn current(protocol_version: u8) -> Self {
        ServerCapabilities {
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version,
            capabilities: Capability::implemented(),
        }
    }

    /// Whether a given capability is advertised.
    pub fn has(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implemented_does_not_include_clustering() {
        // There is intentionally no clustering capability variant.
        let caps = Capability::implemented();
        assert!(caps.contains(&Capability::PersistentStorage));
        assert!(caps.contains(&Capability::VectorExactSearch));
    }

    #[test]
    fn capabilities_roundtrip() {
        let caps = ServerCapabilities::current(1);
        let json = serde_json::to_string(&caps).unwrap();
        let back: ServerCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps, back);
        assert!(back.has(Capability::Explain));
    }
}
