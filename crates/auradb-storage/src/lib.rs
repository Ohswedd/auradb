//! # auradb-storage
//!
//! AuraDB's append-only storage engine. Records are written as atomic,
//! checksummed [`Batch`] frames into numbered segment files described by a
//! [`Manifest`]. On open, segments are replayed to rebuild an in-memory record
//! map; a torn trailing batch is truncated, and a checksum failure on a fully
//! present batch fails closed as [`auradb_core::Error::Corruption`].
//!
//! Identity is logical ([`RecordId`]); physical offsets are never exposed as
//! durable identity (see `docs/STORAGE_ENGINE.md`).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod catalog;
mod format;
mod manifest;

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use auradb_core::{CollectionId, CollectionSchema, Error, Record, RecordId, Result, TxnId};

pub use catalog::Catalog;
pub use format::{Batch, LogOp, ParsedSegment};
pub use manifest::{Manifest, SegmentRef, FORMAT_VERSION};

const MANIFEST_FILE: &str = "MANIFEST";
const CATALOG_FILE: &str = "catalog.json";

/// Tuning options for the storage engine.
#[derive(Debug, Clone)]
pub struct StorageOptions {
    /// Whether to fsync the active segment after every committed batch. Safe by
    /// default; may be disabled for bulk import / benchmarks where the caller
    /// accepts the durability trade-off.
    pub sync_on_commit: bool,
}

impl Default for StorageOptions {
    fn default() -> Self {
        StorageOptions {
            sync_on_commit: true,
        }
    }
}

type RecordMap = BTreeMap<CollectionId, BTreeMap<RecordId, Record>>;

/// The storage engine: a persistent, recoverable append-only record store.
pub struct Storage {
    dir: PathBuf,
    options: StorageOptions,
    manifest: Manifest,
    catalog: Catalog,
    records: RecordMap,
    active_file: File,
    max_txn_id: u64,
}

impl Storage {
    /// Open (creating if necessary) the database at `dir` with default options.
    pub fn open(dir: impl AsRef<Path>) -> Result<Storage> {
        Storage::open_with(dir, StorageOptions::default())
    }

    /// Open (creating if necessary) the database at `dir`.
    pub fn open_with(dir: impl AsRef<Path>, options: StorageOptions) -> Result<Storage> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;

        let manifest_path = dir.join(MANIFEST_FILE);
        let manifest = if manifest_path.exists() {
            Manifest::load(&manifest_path)?
        } else {
            let m = Manifest::default();
            m.save(&manifest_path)?;
            m
        };

        let catalog = Catalog::load(&dir.join(CATALOG_FILE))?;

        let mut records: RecordMap = BTreeMap::new();
        let mut max_txn_id = 0u64;
        let segments = manifest.segments.clone();
        let last_index = segments.len() - 1;
        for (i, seg) in segments.iter().enumerate() {
            let path = dir.join(seg.file_name());
            let buf = if path.exists() {
                fs::read(&path)?
            } else {
                Vec::new()
            };
            let parsed = format::parse_segment(&buf)?;
            if parsed.truncated {
                if i != last_index {
                    return Err(Error::Corruption(format!(
                        "sealed segment {} has a torn batch",
                        seg.id
                    )));
                }
                // Truncate the torn trailing batch from the active segment.
                let f = OpenOptions::new().write(true).open(&path)?;
                f.set_len(parsed.valid_len as u64)?;
                f.sync_all()?;
            }
            for batch in parsed.batches {
                max_txn_id = max_txn_id.max(batch.txn_id.get());
                apply_batch(&mut records, batch);
            }
        }

        // Ensure every catalogued collection has a (possibly empty) map entry.
        for name in catalog.schemas.keys() {
            records.entry(CollectionId::new(name.clone())).or_default();
        }

        let active_path = dir.join(manifest.active().file_name());
        let active_file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&active_path)?;

        Ok(Storage {
            dir,
            options,
            manifest,
            catalog,
            records,
            active_file,
            max_txn_id,
        })
    }

    /// The highest transaction id durably recorded (for clock seeding).
    pub fn max_txn_id(&self) -> u64 {
        self.max_txn_id
    }

    /// The monotonic schema catalog version.
    pub fn schema_version(&self) -> u64 {
        self.catalog.schema_version
    }

    // --- schema catalog ---

    /// Register or replace a collection schema and persist the catalog.
    pub fn put_schema(&mut self, schema: CollectionSchema) -> Result<()> {
        schema.validate_definition()?;
        let name = schema.name.clone();
        self.catalog.put(schema);
        self.catalog.save(&self.dir.join(CATALOG_FILE))?;
        self.records.entry(CollectionId::new(name)).or_default();
        Ok(())
    }

    /// Fetch a schema by collection name.
    pub fn get_schema(&self, name: &str) -> Option<&CollectionSchema> {
        self.catalog.schemas.get(name)
    }

    /// List all registered schemas.
    pub fn list_schemas(&self) -> Vec<&CollectionSchema> {
        self.catalog.schemas.values().collect()
    }

    /// Drop a schema and all of its records durably.
    pub fn drop_schema(&mut self, name: &str) -> Result<()> {
        if !self.catalog.schemas.contains_key(name) {
            return Err(Error::NotFound(format!("collection {name}")));
        }
        let collection = CollectionId::new(name.to_string());
        let ids: Vec<RecordId> = self
            .records
            .get(&collection)
            .map(|m| m.keys().copied().collect())
            .unwrap_or_default();
        if !ids.is_empty() {
            let ops = ids
                .into_iter()
                .map(|id| LogOp::Delete {
                    collection: collection.clone(),
                    id,
                })
                .collect();
            self.commit_batch(Batch {
                txn_id: TxnId::AUTO,
                ops,
            })?;
        }
        self.records.remove(&collection);
        self.catalog.remove(name);
        self.catalog.save(&self.dir.join(CATALOG_FILE))?;
        Ok(())
    }

    // --- data ---

    /// Append a committed batch to the log and apply it in memory.
    ///
    /// The batch is durable (and atomic) once this returns, fsynced when
    /// `sync_on_commit` is enabled.
    pub fn commit_batch(&mut self, batch: Batch) -> Result<()> {
        if batch.ops.is_empty() {
            return Ok(());
        }
        let bytes = batch.encode();
        self.active_file.write_all(&bytes)?;
        self.active_file.flush()?;
        if self.options.sync_on_commit {
            self.active_file.sync_all()?;
        }
        self.max_txn_id = self.max_txn_id.max(batch.txn_id.get());
        apply_batch(&mut self.records, batch);
        Ok(())
    }

    /// Put a single record under an auto-commit batch.
    pub fn put(&mut self, record: Record) -> Result<()> {
        self.commit_batch(Batch {
            txn_id: TxnId::AUTO,
            ops: vec![LogOp::Put { record }],
        })
    }

    /// Delete a single record under an auto-commit batch.
    pub fn delete(&mut self, collection: &CollectionId, id: RecordId) -> Result<()> {
        self.commit_batch(Batch {
            txn_id: TxnId::AUTO,
            ops: vec![LogOp::Delete {
                collection: collection.clone(),
                id,
            }],
        })
    }

    /// Fetch a record by collection and id.
    pub fn get(&self, collection: &CollectionId, id: RecordId) -> Option<&Record> {
        self.records.get(collection)?.get(&id)
    }

    /// Iterate over all live records in a collection (empty if unknown).
    pub fn scan(&self, collection: &CollectionId) -> impl Iterator<Item = &Record> {
        self.records
            .get(collection)
            .into_iter()
            .flat_map(|m| m.values())
    }

    /// The number of live records in a collection.
    pub fn count(&self, collection: &CollectionId) -> usize {
        self.records.get(collection).map(|m| m.len()).unwrap_or(0)
    }

    /// The total number of live records across all collections.
    pub fn total_records(&self) -> usize {
        self.records.values().map(|m| m.len()).sum()
    }

    /// The number of registered collections.
    pub fn collection_count(&self) -> usize {
        self.catalog.schemas.len()
    }

    /// Force durability of the active segment and manifest.
    pub fn flush(&mut self) -> Result<()> {
        self.active_file.flush()?;
        self.active_file.sync_all()?;
        self.manifest.save(&self.dir.join(MANIFEST_FILE))?;
        Ok(())
    }

    /// Compact the database: rewrite all live records into a single fresh
    /// segment and drop the old segments. Preserves all live data.
    pub fn compact(&mut self) -> Result<CompactionReport> {
        let before_segments = self.manifest.segments.len();
        let ops: Vec<LogOp> = self
            .records
            .values()
            .flat_map(|m| m.values())
            .map(|r| LogOp::Put { record: r.clone() })
            .collect();
        let record_count = ops.len();

        let new_id = self.manifest.next_segment_id;
        let new_seg = SegmentRef { id: new_id };
        let new_path = self.dir.join(new_seg.file_name());
        let mut new_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&new_path)?;
        if !ops.is_empty() {
            let batch = Batch {
                txn_id: TxnId(self.max_txn_id),
                ops,
            };
            new_file.write_all(&batch.encode())?;
        }
        new_file.sync_all()?;

        let old_segments = std::mem::replace(&mut self.manifest.segments, vec![new_seg]);
        self.manifest.next_segment_id = new_id + 1;
        self.manifest.save(&self.dir.join(MANIFEST_FILE))?;

        for seg in &old_segments {
            let path = self.dir.join(seg.file_name());
            let _ = fs::remove_file(path);
        }

        self.active_file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&new_path)?;

        Ok(CompactionReport {
            segments_before: before_segments,
            segments_after: 1,
            live_records: record_count,
        })
    }
}

/// The result of a compaction run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionReport {
    /// Segment count before compaction.
    pub segments_before: usize,
    /// Segment count after compaction (always 1).
    pub segments_after: usize,
    /// Number of live records retained.
    pub live_records: usize,
}

fn apply_batch(records: &mut RecordMap, batch: Batch) {
    for op in batch.ops {
        match op {
            LogOp::Put { record } => {
                records
                    .entry(record.collection.clone())
                    .or_default()
                    .insert(record.id, record);
            }
            LogOp::Delete { collection, id } => {
                if let Some(m) = records.get_mut(&collection) {
                    m.remove(&id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auradb_core::{Document, FieldDef, FieldType, Value};

    fn rec(id: u128, v: i64) -> Record {
        let mut fields = Document::new();
        fields.insert("v".into(), Value::Int(v));
        Record::new(RecordId::from_u128(id), CollectionId::new("C"), fields)
    }

    #[test]
    fn write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Storage::open(dir.path()).unwrap();
        s.put(rec(1, 10)).unwrap();
        let got = s
            .get(&CollectionId::new("C"), RecordId::from_u128(1))
            .unwrap();
        assert_eq!(got.get("v"), Some(&Value::Int(10)));
    }

    #[test]
    fn delete_removes() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Storage::open(dir.path()).unwrap();
        s.put(rec(1, 10)).unwrap();
        s.delete(&CollectionId::new("C"), RecordId::from_u128(1))
            .unwrap();
        assert!(s
            .get(&CollectionId::new("C"), RecordId::from_u128(1))
            .is_none());
    }

    #[test]
    fn restart_persistence() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut s = Storage::open(dir.path()).unwrap();
            s.put(rec(1, 10)).unwrap();
            s.put(rec(2, 20)).unwrap();
            s.delete(&CollectionId::new("C"), RecordId::from_u128(1))
                .unwrap();
        }
        let s = Storage::open(dir.path()).unwrap();
        assert!(s
            .get(&CollectionId::new("C"), RecordId::from_u128(1))
            .is_none());
        assert_eq!(
            s.get(&CollectionId::new("C"), RecordId::from_u128(2))
                .unwrap()
                .get("v"),
            Some(&Value::Int(20))
        );
    }

    #[test]
    fn scan_and_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Storage::open(dir.path()).unwrap();
        for i in 1..=5 {
            s.put(rec(i, i as i64)).unwrap();
        }
        assert_eq!(s.count(&CollectionId::new("C")), 5);
        assert_eq!(s.scan(&CollectionId::new("C")).count(), 5);
    }

    #[test]
    fn schema_catalog_persists() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut s = Storage::open(dir.path()).unwrap();
            s.put_schema(
                CollectionSchema::new("User").with_field(FieldDef::new("id", FieldType::Uuid)),
            )
            .unwrap();
        }
        let s = Storage::open(dir.path()).unwrap();
        assert!(s.get_schema("User").is_some());
        assert_eq!(s.collection_count(), 1);
    }

    #[test]
    fn checksum_corruption_detected() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut s = Storage::open(dir.path()).unwrap();
            s.put(rec(1, 10)).unwrap();
        }
        let seg = dir.path().join("0000000001.seg");
        let mut bytes = fs::read(&seg).unwrap();
        let mid = bytes.len() - 1;
        bytes[mid] ^= 0xff;
        fs::write(&seg, &bytes).unwrap();
        assert!(matches!(
            Storage::open(dir.path()),
            Err(Error::Corruption(_))
        ));
    }

    #[test]
    fn compaction_preserves_live_data() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Storage::open(dir.path()).unwrap();
        for i in 1..=10 {
            s.put(rec(i, i as i64)).unwrap();
        }
        s.put(rec(1, 999)).unwrap();
        s.delete(&CollectionId::new("C"), RecordId::from_u128(2))
            .unwrap();
        let report = s.compact().unwrap();
        assert_eq!(report.live_records, 9);
        assert_eq!(report.segments_after, 1);
        assert_eq!(
            s.get(&CollectionId::new("C"), RecordId::from_u128(1))
                .unwrap()
                .get("v"),
            Some(&Value::Int(999))
        );
        assert!(s
            .get(&CollectionId::new("C"), RecordId::from_u128(2))
            .is_none());

        drop(s);
        let s = Storage::open(dir.path()).unwrap();
        assert_eq!(s.count(&CollectionId::new("C")), 9);
    }

    #[test]
    fn drop_schema_removes_records() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Storage::open(dir.path()).unwrap();
        s.put_schema(CollectionSchema::new("C").with_field(FieldDef::new("v", FieldType::Int)))
            .unwrap();
        s.put(rec(1, 1)).unwrap();
        s.drop_schema("C").unwrap();
        assert!(s.get_schema("C").is_none());
        drop(s);
        let s = Storage::open(dir.path()).unwrap();
        assert_eq!(s.count(&CollectionId::new("C")), 0);
    }
}
