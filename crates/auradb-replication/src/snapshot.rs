//! The snapshot boundary for future state transfer.
//!
//! v0.4.0 does **not** ship streaming snapshot transfer between nodes. What it
//! does ship is the *boundary*: a versioned [`SnapshotManifest`] that names the
//! log index a snapshot covers and carries a content digest, plus the create /
//! restore seam that captures and rebuilds engine state. Defining this now means
//! a later release can add over-the-wire snapshot shipping without another
//! on-disk format change.
//!
//! The snapshot payload is a portable logical dump (schemas + current live
//! records) captured through the engine's public API, so a restore rebuilds
//! storage, indexes, and planner statistics exactly as a normal load would.

use serde::{Deserialize, Serialize};

use auradb::core::{CollectionSchema, Document};
use auradb::query::{FindQuery, Mutation};
use auradb::Engine;

use crate::error::{ReplicationError, Result};

/// The snapshot manifest format version this build writes and understands.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

/// Metadata describing a state-machine snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMeta {
    /// Manifest format version (rejected if newer than this build supports).
    pub format_version: u32,
    /// The last Raft log index included in the snapshot. Log entries at or below
    /// this index may be compacted once a snapshot is durable.
    pub last_included_index: u64,
    /// The Raft term of `last_included_index`.
    pub last_included_term: u64,
    /// A digest of the snapshot payload, for integrity checks.
    pub digest: u32,
    /// The AuraDB version that wrote the snapshot.
    pub created_by_version: String,
}

/// The logical contents of a snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
struct SnapshotData {
    schemas: Vec<CollectionSchema>,
    records: Vec<(String, Document)>,
}

/// A snapshot manifest plus its serialized payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// The snapshot metadata.
    pub meta: SnapshotMeta,
    /// The serialized engine state (a portable logical dump).
    pub payload: Vec<u8>,
}

impl SnapshotManifest {
    /// Capture the engine's current state as a snapshot covering up to
    /// `last_included_index` / `last_included_term`.
    pub fn create(
        engine: &Engine,
        last_included_index: u64,
        last_included_term: u64,
        version: &str,
    ) -> Result<SnapshotManifest> {
        let mut data = SnapshotData::default();
        let schemas = engine.list_schemas();
        for schema in &schemas {
            let rows = engine
                .find(&FindQuery::new(&schema.name))
                .map_err(|e| ReplicationError::SnapshotMalformed(e.to_string()))?;
            for row in rows {
                data.records.push((schema.name.clone(), row.fields));
            }
        }
        data.schemas = schemas;
        let payload = serde_json::to_vec(&data)
            .map_err(|e| ReplicationError::SnapshotMalformed(e.to_string()))?;
        let digest = crc32fast::hash(&payload);
        Ok(SnapshotManifest {
            meta: SnapshotMeta {
                format_version: SNAPSHOT_FORMAT_VERSION,
                last_included_index,
                last_included_term,
                digest,
                created_by_version: version.to_string(),
            },
            payload,
        })
    }

    /// Validate the manifest's version and integrity, returning the payload.
    pub fn verified_payload(&self) -> Result<&[u8]> {
        if self.meta.format_version > SNAPSHOT_FORMAT_VERSION {
            return Err(ReplicationError::UnsupportedSnapshot {
                found: self.meta.format_version,
                supported: SNAPSHOT_FORMAT_VERSION,
            });
        }
        if crc32fast::hash(&self.payload) != self.meta.digest {
            return Err(ReplicationError::SnapshotMalformed(
                "snapshot payload digest mismatch".into(),
            ));
        }
        Ok(&self.payload)
    }

    /// Restore a fresh engine at `dir` from this snapshot.
    pub fn restore(&self, dir: impl AsRef<std::path::Path>) -> Result<Engine> {
        let payload = self.verified_payload()?;
        let data: SnapshotData = serde_json::from_slice(payload)
            .map_err(|e| ReplicationError::SnapshotMalformed(e.to_string()))?;
        let engine = Engine::open(dir.as_ref())?;
        for schema in data.schemas {
            engine.create_schema(schema)?;
        }
        for (collection, fields) in data.records {
            engine.apply_mutation(Mutation::Upsert { collection, fields })?;
        }
        Ok(engine)
    }

    /// Encode the manifest to bytes (for storage or transfer).
    pub fn encode(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| ReplicationError::SnapshotMalformed(e.to_string()))
    }

    /// Decode a manifest from bytes, rejecting unknown future versions.
    pub fn decode(bytes: &[u8]) -> Result<SnapshotManifest> {
        let manifest: SnapshotManifest = serde_json::from_slice(bytes)
            .map_err(|e| ReplicationError::SnapshotMalformed(e.to_string()))?;
        if manifest.meta.format_version > SNAPSHOT_FORMAT_VERSION {
            return Err(ReplicationError::UnsupportedSnapshot {
                found: manifest.meta.format_version,
                supported: SNAPSHOT_FORMAT_VERSION,
            });
        }
        Ok(manifest)
    }
}
