//! The snapshot boundary for local capture, inspection, and restore.
//!
//! v0.4.1 does **not** ship streaming snapshot transfer between nodes. What it
//! does ship is the *boundary* — hardened in this release: a versioned
//! [`SnapshotManifest`] that names the log index a snapshot covers, records the
//! cluster/node identity and storage format it was taken from, and carries a
//! content digest; plus a create / inspect / restore seam that captures and
//! rebuilds engine state. Defining this now means a later release can add
//! over-the-wire snapshot shipping without another on-disk format change.
//!
//! The snapshot payload is a portable logical dump (schemas + current live
//! records) captured through the engine's public API, so a restore rebuilds
//! storage, indexes, and planner statistics exactly as a normal load would.
//!
//! ## Restore safety
//!
//! [`SnapshotManifest::restore_to`] is **atomic**: it materializes the snapshot
//! into a staging directory beside the target, validates it, and only then swaps
//! it into place. If anything fails — a future format, a digest mismatch, a
//! cluster-id mismatch, or an apply error — the existing target directory is left
//! untouched. Restore refuses to overwrite a non-empty target unless `force` is
//! set.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use auradb::core::{CollectionSchema, Document};
use auradb::query::{FindQuery, Mutation};
use auradb::Engine;

use crate::error::{ReplicationError, Result};

/// The snapshot manifest format version this build writes and understands.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

/// Metadata describing a state-machine snapshot.
///
/// New identity and provenance fields added in v0.4.1 are optional and default
/// to absent, so a manifest written by v0.4.0 still decodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMeta {
    /// Manifest format version (rejected if newer than this build supports).
    pub format_version: u32,
    /// The cluster id (hex) the snapshot was taken from, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<String>,
    /// The node id (hex) the snapshot was taken from, for a local snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// The last Raft log index included in the snapshot. Log entries at or below
    /// this index may be compacted once a snapshot is durable.
    pub last_included_index: u64,
    /// The Raft term of `last_included_index`.
    pub last_included_term: u64,
    /// The storage format version the snapshot was captured from. A restore into
    /// a build that cannot read this format is refused.
    #[serde(default = "default_storage_format")]
    pub storage_format_version: u32,
    /// The number of collections captured (a quick integrity cross-check).
    #[serde(default)]
    pub collections: usize,
    /// The number of records captured.
    #[serde(default)]
    pub records: usize,
    /// A digest of the snapshot payload, for integrity checks.
    pub digest: u32,
    /// The AuraDB version that wrote the snapshot.
    pub created_by_version: String,
    /// Unix seconds at which the snapshot was created (`0` if not recorded).
    #[serde(default)]
    pub created_at_unix: u64,
}

fn default_storage_format() -> u32 {
    auradb_storage::FORMAT_VERSION
}

/// Options controlling a snapshot restore.
#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    /// Overwrite a non-empty target directory. Without this, restore into a
    /// non-empty directory is refused so existing data is never clobbered.
    pub force: bool,
    /// The cluster id the target is expected to belong to. When set and the
    /// snapshot records a different cluster id, restore is refused unless
    /// [`allow_cluster_mismatch`](Self::allow_cluster_mismatch) is set.
    pub expected_cluster_id: Option<String>,
    /// Permit restoring a snapshot whose cluster id differs from the expected one.
    pub allow_cluster_mismatch: bool,
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
        let collections = data.schemas.len();
        let records = data.records.len();
        let payload = serde_json::to_vec(&data)
            .map_err(|e| ReplicationError::SnapshotMalformed(e.to_string()))?;
        let digest = crc32fast::hash(&payload);
        Ok(SnapshotManifest {
            meta: SnapshotMeta {
                format_version: SNAPSHOT_FORMAT_VERSION,
                cluster_id: None,
                node_id: None,
                last_included_index,
                last_included_term,
                storage_format_version: auradb_storage::FORMAT_VERSION,
                collections,
                records,
                digest,
                created_by_version: version.to_string(),
                created_at_unix: now_unix(),
            },
            payload,
        })
    }

    /// Record the cluster and node identity this snapshot was taken from. Used
    /// when capturing a local cluster snapshot so a later restore can detect a
    /// cluster-id mismatch.
    pub fn with_identity(mut self, cluster_id: Option<String>, node_id: Option<String>) -> Self {
        self.meta.cluster_id = cluster_id;
        self.meta.node_id = node_id;
        self
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

    /// Restore a fresh engine at `dir` from this snapshot (non-atomic; assumes
    /// `dir` does not yet exist or is empty). Prefer [`restore_to`](Self::restore_to)
    /// for the safe, atomic path.
    pub fn restore(&self, dir: impl AsRef<Path>) -> Result<Engine> {
        self.materialize_into(dir.as_ref())
    }

    /// Atomically restore this snapshot into `dir`.
    ///
    /// Validates the manifest, refuses to overwrite a non-empty target unless
    /// `opts.force` is set, refuses a cluster-id mismatch unless allowed, and
    /// rejects a storage format newer than this build. The snapshot is built in a
    /// staging directory beside the target and swapped into place only after it
    /// validates, so a failure never corrupts existing data.
    pub fn restore_to(&self, dir: impl AsRef<Path>, opts: &RestoreOptions) -> Result<Engine> {
        let target = dir.as_ref();
        self.validate_for_restore(opts)?;

        if dir_is_nonempty(target) && !opts.force {
            return Err(ReplicationError::SnapshotRestoreRefused(format!(
                "target directory {} is not empty; pass --force to overwrite it",
                target.display()
            )));
        }

        let staging = staging_path(target);
        let _ = std::fs::remove_dir_all(&staging);
        // Build and validate into staging. On any error, clean up and leave the
        // existing target untouched.
        match self.materialize_into(&staging) {
            Ok(engine) => {
                // Close the engine before swapping directories.
                drop(engine);
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        }

        // Swap: remove the old target (if any), then rename staging into place.
        if target.exists() {
            std::fs::remove_dir_all(target).map_err(|e| {
                ReplicationError::SnapshotMalformed(format!("removing previous target: {e}"))
            })?;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ReplicationError::SnapshotMalformed(e.to_string()))?;
        }
        std::fs::rename(&staging, target).map_err(|e| {
            ReplicationError::SnapshotMalformed(format!("swapping restored data into place: {e}"))
        })?;
        Engine::open(target).map_err(ReplicationError::Apply)
    }

    /// Validate everything that can be checked before touching the filesystem:
    /// format version, payload digest, storage format, and cluster id.
    fn validate_for_restore(&self, opts: &RestoreOptions) -> Result<()> {
        self.verified_payload()?;
        if self.meta.storage_format_version > auradb_storage::FORMAT_VERSION {
            return Err(ReplicationError::SnapshotRestoreRefused(format!(
                "snapshot was captured from storage format v{} but this build supports up to v{}",
                self.meta.storage_format_version,
                auradb_storage::FORMAT_VERSION
            )));
        }
        if let (Some(expected), Some(found)) = (&opts.expected_cluster_id, &self.meta.cluster_id) {
            if expected != found && !opts.allow_cluster_mismatch {
                return Err(ReplicationError::SnapshotRestoreRefused(format!(
                    "snapshot belongs to cluster {found} but the target is cluster {expected}; \
                     pass an explicit override to restore across clusters"
                )));
            }
        }
        Ok(())
    }

    /// Materialize the snapshot's schemas and records into a fresh engine at
    /// `dir`. Rebuilds storage, indexes, and planner statistics through the
    /// engine's normal load path.
    fn materialize_into(&self, dir: &Path) -> Result<Engine> {
        let payload = self.verified_payload()?;
        let data: SnapshotData = serde_json::from_slice(payload)
            .map_err(|e| ReplicationError::SnapshotMalformed(e.to_string()))?;
        let engine = Engine::open(dir).map_err(ReplicationError::Apply)?;
        for schema in data.schemas {
            engine
                .create_schema(schema)
                .map_err(ReplicationError::Apply)?;
        }
        for (collection, fields) in data.records {
            engine
                .apply_mutation(Mutation::Upsert { collection, fields })
                .map_err(ReplicationError::Apply)?;
        }
        // Refresh planner statistics so a restored engine plans like a loaded one.
        engine.analyze().map_err(ReplicationError::Apply)?;
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

/// A staging directory beside `target` for an atomic restore.
fn staging_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "snapshot".to_string());
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!(".{name}.restore-staging-{}", std::process::id()))
}

/// Whether `dir` exists and contains at least one entry.
fn dir_is_nonempty(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
