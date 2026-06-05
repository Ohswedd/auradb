//! The manifest: the durable description of the database's segment files.

use std::fs;
use std::path::Path;

use auradb_core::{Error, Result};
use serde::{Deserialize, Serialize};

/// The on-disk storage format version. Bumped on incompatible layout changes.
///
/// - **v1** (AuraDB ≤ 0.2.x): single live record per id; no commit timestamps.
/// - **v2** (AuraDB ≥ 0.3.0): MVCC version chains with per-op commit timestamps.
///
/// A v1 database is migrated to v2 transparently on first open (see
/// [`crate::Storage::open_with`]); a format version newer than this build's
/// `FORMAT_VERSION` is rejected.
pub const FORMAT_VERSION: u32 = 2;

/// The oldest on-disk format this build can read (and migrate forward).
pub const MIN_READABLE_FORMAT_VERSION: u32 = 1;

/// A reference to one segment file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentRef {
    /// The numeric segment id, also its file stem.
    pub id: u64,
}

impl SegmentRef {
    /// The file name for this segment.
    pub fn file_name(&self) -> String {
        format!("{:010}.seg", self.id)
    }
}

/// The durable manifest describing all live segments and recovery metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Storage format version.
    pub format_version: u32,
    /// Ordered list of live segments (writes target the last one).
    pub segments: Vec<SegmentRef>,
    /// The next segment id to allocate.
    pub next_segment_id: u64,
    /// The highest transaction id durably recorded.
    pub last_txn_id: u64,
    /// The highest MVCC commit timestamp durably recorded. Defaults to `0` when
    /// reading a v1 manifest; the engine reseeds it during migration.
    #[serde(default)]
    pub last_commit_ts: u64,
}

impl Default for Manifest {
    fn default() -> Self {
        Manifest {
            format_version: FORMAT_VERSION,
            segments: vec![SegmentRef { id: 1 }],
            next_segment_id: 2,
            last_txn_id: 0,
            last_commit_ts: 0,
        }
    }
}

impl Manifest {
    /// The active (write) segment.
    pub fn active(&self) -> &SegmentRef {
        self.segments
            .last()
            .expect("manifest always has at least one segment")
    }

    /// Load a manifest from `path`.
    pub fn load(path: &Path) -> Result<Manifest> {
        let bytes = fs::read(path)?;
        let manifest: Manifest = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Corruption(format!("manifest is malformed: {e}")))?;
        // Accept any format in [MIN_READABLE_FORMAT_VERSION, FORMAT_VERSION]: an
        // older format is migrated forward on open; a newer one (a future build's
        // data) is rejected rather than silently misread.
        if manifest.format_version > FORMAT_VERSION
            || manifest.format_version < MIN_READABLE_FORMAT_VERSION
        {
            return Err(Error::unsupported(format!(
                "storage format version {} (this build supports {MIN_READABLE_FORMAT_VERSION}..={FORMAT_VERSION})",
                manifest.format_version
            )));
        }
        if manifest.segments.is_empty() {
            return Err(Error::Corruption("manifest lists no segments".into()));
        }
        Ok(manifest)
    }

    /// Atomically persist the manifest to `path` (write temp + rename + fsync).
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::Storage(format!("manifest serialization failed: {e}")))?;
        fs::write(&tmp, &bytes)?;
        // fsync the temp file before rename for durability.
        let f = fs::File::open(&tmp)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("MANIFEST");
        let mut m = Manifest {
            last_txn_id: 9,
            next_segment_id: 3,
            ..Manifest::default()
        };
        m.segments.push(SegmentRef { id: 2 });
        m.save(&path).unwrap();
        let back = Manifest::load(&path).unwrap();
        assert_eq!(m, back);
        assert_eq!(back.active().id, 2);
    }

    #[test]
    fn segment_file_name_is_zero_padded() {
        assert_eq!(SegmentRef { id: 7 }.file_name(), "0000000007.seg");
    }

    #[test]
    fn malformed_manifest_is_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("MANIFEST");
        std::fs::write(&path, b"not json").unwrap();
        assert!(matches!(Manifest::load(&path), Err(Error::Corruption(_))));
    }
}
