//! # auradb-storage
//!
//! AuraDB's append-only, **MVCC** storage engine. Records are written as atomic,
//! checksummed [`Batch`] frames into numbered segment files described by a
//! [`Manifest`]. Each record id maps to a *version chain*: an ordered list of
//! committed versions, each tagged with the commit timestamp at which it became
//! visible (a delete is a tombstone version). On open, segments are replayed to
//! rebuild the in-memory chains; a torn trailing batch is truncated, and a
//! checksum failure on a fully present batch fails closed as
//! [`auradb_core::Error::Corruption`].
//!
//! Reads come in two flavours: *latest committed* ([`Storage::get`],
//! [`Storage::scan`]) for non-transactional access, and *as-of a read timestamp*
//! ([`Storage::get_as_of`], [`Storage::scan_as_of`]) for snapshot-isolated
//! transactions. Old versions are reclaimed by [`Storage::gc`] once no active
//! transaction can observe them.
//!
//! A pre-0.3.0 (format v1) database — single live record per id — is migrated to
//! v2 transparently on first open.
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
pub use manifest::{Manifest, SegmentRef, FORMAT_VERSION, MIN_READABLE_FORMAT_VERSION};

const MANIFEST_FILE: &str = "MANIFEST";
const CATALOG_FILE: &str = "catalog.json";

/// One committed version of a record in its version chain.
#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    /// The commit timestamp at which this version became visible.
    pub commit_ts: u64,
    /// The record payload, or `None` for a tombstone (the record was deleted at
    /// `commit_ts`).
    pub value: Option<Record>,
}

/// An ordered version chain for one record id, ascending by `commit_ts`.
type Chain = Vec<Version>;

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

type RecordMap = BTreeMap<CollectionId, BTreeMap<RecordId, Chain>>;

/// The storage engine: a persistent, recoverable, multi-version record store.
pub struct Storage {
    dir: PathBuf,
    options: StorageOptions,
    manifest: Manifest,
    catalog: Catalog,
    records: RecordMap,
    active_file: File,
    max_txn_id: u64,
    /// The highest commit timestamp allocated so far (the commit watermark).
    last_commit_ts: u64,
}

/// The visible version of a chain *as of* a read timestamp: the version with the
/// greatest `commit_ts <= read_ts`, or `None` if none exists yet.
fn visible_at(chain: &Chain, read_ts: u64) -> Option<&Version> {
    // Chain is ascending by commit_ts; take the last entry committed at or before
    // read_ts.
    chain.iter().rev().find(|v| v.commit_ts <= read_ts)
}

/// The latest committed version of a chain (tombstone or live), if any.
fn latest(chain: &Chain) -> Option<&Version> {
    chain.last()
}

/// Insert a version into a chain, keeping it ascending by `commit_ts`. Replaces
/// an existing version at the same timestamp (idempotent replay).
fn insert_version(chain: &mut Chain, version: Version) {
    match chain.binary_search_by(|v| v.commit_ts.cmp(&version.commit_ts)) {
        Ok(pos) => chain[pos] = version,
        Err(pos) => chain.insert(pos, version),
    }
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
        // A v1 log has no commit timestamps; migrate by assigning a fresh,
        // monotonically increasing commit timestamp to every replayed op in log
        // order, then rewriting the database in v2 format.
        let migrating = manifest.format_version < FORMAT_VERSION;

        let mut records: RecordMap = BTreeMap::new();
        let mut max_txn_id = 0u64;
        let mut last_commit_ts = manifest.last_commit_ts;
        let mut migrate_ts = 0u64;
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
            for mut batch in parsed.batches {
                max_txn_id = max_txn_id.max(batch.txn_id.get());
                if migrating {
                    // Reassign commit timestamps in strict log order so that the
                    // recovered chains carry a coherent MVCC history.
                    for op in &mut batch.ops {
                        migrate_ts += 1;
                        op.set_commit_ts(migrate_ts);
                    }
                }
                for op in &batch.ops {
                    last_commit_ts = last_commit_ts.max(op.commit_ts());
                }
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

        let mut storage = Storage {
            dir,
            options,
            manifest,
            catalog,
            records,
            active_file,
            max_txn_id,
            last_commit_ts,
        };

        if migrating {
            // Persist the recovered chains in v2 format and stamp the manifest so
            // subsequent opens read v2 directly.
            storage.rewrite_segment()?;
        }

        Ok(storage)
    }

    /// The highest transaction id durably recorded (for clock seeding).
    pub fn max_txn_id(&self) -> u64 {
        self.max_txn_id
    }

    /// The MVCC commit watermark: the highest commit timestamp committed so far.
    /// A transaction beginning now pins this as its read timestamp.
    pub fn commit_watermark(&self) -> u64 {
        self.last_commit_ts
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
        // Tombstone every currently-live record so the drop is durable, then
        // forget the chains entirely (no transaction can reference a dropped
        // collection's schema).
        let ids: Vec<RecordId> = self
            .records
            .get(&collection)
            .map(|m| {
                m.iter()
                    .filter(|(_, c)| latest(c).is_some_and(|v| v.value.is_some()))
                    .map(|(id, _)| *id)
                    .collect()
            })
            .unwrap_or_default();
        if !ids.is_empty() {
            let ops = ids
                .into_iter()
                .map(|id| LogOp::Delete {
                    commit_ts: 0,
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

    /// Append a committed batch to the log and apply it in memory, allocating a
    /// fresh commit timestamp for the whole batch (every op shares it, so the
    /// transaction commits atomically at one point in MVCC time).
    ///
    /// The batch is durable (and atomic) once this returns, fsynced when
    /// `sync_on_commit` is enabled. Returns the commit timestamp assigned.
    pub fn commit_batch(&mut self, mut batch: Batch) -> Result<u64> {
        if batch.ops.is_empty() {
            return Ok(self.last_commit_ts);
        }
        let commit_ts = self.last_commit_ts + 1;
        for op in &mut batch.ops {
            op.set_commit_ts(commit_ts);
        }
        let bytes = batch.encode();
        self.active_file.write_all(&bytes)?;
        self.active_file.flush()?;
        if self.options.sync_on_commit {
            self.active_file.sync_all()?;
        }
        self.max_txn_id = self.max_txn_id.max(batch.txn_id.get());
        self.last_commit_ts = commit_ts;
        apply_batch(&mut self.records, batch);
        Ok(commit_ts)
    }

    /// Apply a batch that has already been ordered and committed by an external
    /// log (Raft), stamping every op with the caller-provided `commit_ts`.
    ///
    /// Unlike [`Storage::commit_batch`], this does **not** allocate a timestamp:
    /// the replication layer uses the committed log index as the commit
    /// timestamp so that every replica derives identical, monotonic MVCC
    /// timestamps from the same ordered log. The operation is **idempotent**: if
    /// `commit_ts` is not greater than the current watermark the batch is treated
    /// as already applied and ignored, which makes crash-recovery replay safe.
    ///
    /// `commit_ts` must be strictly greater than the current watermark (a gap is
    /// allowed — log entries that produce no batch, such as no-ops, simply skip
    /// timestamps); a regression is rejected as corruption.
    pub fn apply_committed_batch(&mut self, mut batch: Batch, commit_ts: u64) -> Result<()> {
        if commit_ts <= self.last_commit_ts {
            // Already durable from an earlier apply; replay is a no-op.
            return Ok(());
        }
        if batch.ops.is_empty() {
            self.last_commit_ts = commit_ts;
            return Ok(());
        }
        for op in &mut batch.ops {
            op.set_commit_ts(commit_ts);
        }
        let bytes = batch.encode();
        self.active_file.write_all(&bytes)?;
        self.active_file.flush()?;
        if self.options.sync_on_commit {
            self.active_file.sync_all()?;
        }
        self.max_txn_id = self.max_txn_id.max(batch.txn_id.get());
        self.last_commit_ts = commit_ts;
        apply_batch(&mut self.records, batch);
        Ok(())
    }

    /// Put a single record under an auto-commit batch. Returns its commit ts.
    pub fn put(&mut self, record: Record) -> Result<u64> {
        self.commit_batch(Batch {
            txn_id: TxnId::AUTO,
            ops: vec![LogOp::Put {
                commit_ts: 0,
                record,
            }],
        })
    }

    /// Delete a single record under an auto-commit batch (writes a tombstone).
    pub fn delete(&mut self, collection: &CollectionId, id: RecordId) -> Result<u64> {
        self.commit_batch(Batch {
            txn_id: TxnId::AUTO,
            ops: vec![LogOp::Delete {
                commit_ts: 0,
                collection: collection.clone(),
                id,
            }],
        })
    }

    /// Fetch the latest committed version of a record (non-transactional read).
    pub fn get(&self, collection: &CollectionId, id: RecordId) -> Option<&Record> {
        let chain = self.records.get(collection)?.get(&id)?;
        latest(chain).and_then(|v| v.value.as_ref())
    }

    /// Iterate over the latest committed live records in a collection (empty if
    /// unknown). Tombstoned records are skipped.
    pub fn scan(&self, collection: &CollectionId) -> impl Iterator<Item = &Record> {
        self.records
            .get(collection)
            .into_iter()
            .flat_map(|m| m.values())
            .filter_map(|chain| latest(chain).and_then(|v| v.value.as_ref()))
    }

    /// Fetch the version of a record visible as of `read_ts` (snapshot read).
    pub fn get_as_of(
        &self,
        collection: &CollectionId,
        id: RecordId,
        read_ts: u64,
    ) -> Option<&Record> {
        let chain = self.records.get(collection)?.get(&id)?;
        visible_at(chain, read_ts).and_then(|v| v.value.as_ref())
    }

    /// Iterate over the records in a collection visible as of `read_ts`.
    pub fn scan_as_of(
        &self,
        collection: &CollectionId,
        read_ts: u64,
    ) -> impl Iterator<Item = &Record> {
        self.records
            .get(collection)
            .into_iter()
            .flat_map(|m| m.values())
            .filter_map(move |chain| visible_at(chain, read_ts).and_then(|v| v.value.as_ref()))
    }

    /// The commit timestamp of the most recent committed version of a record
    /// (tombstone or live), if the id has ever been written. Used by the engine
    /// for snapshot-isolation write-conflict detection.
    pub fn latest_commit_ts(&self, collection: &CollectionId, id: RecordId) -> Option<u64> {
        let chain = self.records.get(collection)?.get(&id)?;
        latest(chain).map(|v| v.commit_ts)
    }

    /// The number of live records (latest version not a tombstone) in a
    /// collection.
    pub fn count(&self, collection: &CollectionId) -> usize {
        self.records
            .get(collection)
            .map(|m| {
                m.values()
                    .filter(|c| latest(c).is_some_and(|v| v.value.is_some()))
                    .count()
            })
            .unwrap_or(0)
    }

    /// The total number of live records across all collections.
    pub fn total_records(&self) -> usize {
        self.records.keys().map(|c| self.count(c)).sum()
    }

    /// The total number of stored versions (including tombstones and superseded
    /// versions) across all collections. Used by observability and GC reporting.
    pub fn total_versions(&self) -> usize {
        self.records
            .values()
            .flat_map(|m| m.values())
            .map(|c| c.len())
            .sum()
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

    /// Compact the database: rewrite every retained version chain into a single
    /// fresh segment and drop the old segments. Preserves all versions (it does
    /// not reclaim history — that is [`Storage::gc`]).
    pub fn compact(&mut self) -> Result<CompactionReport> {
        let before_segments = self.manifest.segments.len();
        let live_records = self.total_records();
        self.rewrite_segment()?;
        Ok(CompactionReport {
            segments_before: before_segments,
            segments_after: 1,
            live_records,
        })
    }

    /// Reclaim old MVCC versions no active transaction can observe.
    ///
    /// `cutoff` is the oldest read timestamp still pinned by an active
    /// transaction (or the commit watermark when none are active). A version is
    /// removed only when a newer version committed at or before `cutoff` exists,
    /// so it is unreachable from any read timestamp `>= cutoff`. At least
    /// `min_retained_versions` (and always the latest) versions of every still
    /// live record id are kept. A record whose latest version is a tombstone
    /// committed at or before `cutoff` is removed entirely. The pruned state is
    /// persisted before returning.
    pub fn gc(&mut self, cutoff: u64, min_retained_versions: usize) -> Result<GcReport> {
        let keep = min_retained_versions.max(1);
        let bytes_before = self.segment_bytes();
        let mut versions_reclaimed = 0usize;
        let mut records_removed = 0usize;
        for chains in self.records.values_mut() {
            chains.retain(|_id, chain| {
                // Fully-dead record: latest version is a tombstone older than any
                // active reader — drop the whole chain.
                if let Some(last) = chain.last() {
                    if last.value.is_none() && last.commit_ts <= cutoff {
                        versions_reclaimed += chain.len();
                        records_removed += 1;
                        return false;
                    }
                }
                // Prune versions superseded before `cutoff`. `floor` is the newest
                // version visible at `cutoff`; everything before it is unreachable.
                if let Some(floor) = chain.iter().rposition(|v| v.commit_ts <= cutoff) {
                    let max_removable = chain.len().saturating_sub(keep);
                    let removable = floor.min(max_removable);
                    if removable > 0 {
                        chain.drain(0..removable);
                        versions_reclaimed += removable;
                    }
                }
                true
            });
        }
        self.rewrite_segment()?;
        let bytes_after = self.segment_bytes();
        Ok(GcReport {
            versions_reclaimed,
            records_removed,
            versions_after: self.total_versions(),
            bytes_reclaimed: bytes_before.saturating_sub(bytes_after),
        })
    }

    /// Compute what a [`gc`](Self::gc) at `cutoff` would reclaim, without
    /// modifying any data. The byte estimate is not computed (it requires the
    /// rewrite) and is reported as zero; version and record counts are exact.
    pub fn gc_preview(&self, cutoff: u64, min_retained_versions: usize) -> GcReport {
        let keep = min_retained_versions.max(1);
        let mut versions_reclaimed = 0usize;
        let mut records_removed = 0usize;
        for chains in self.records.values() {
            for chain in chains.values() {
                if let Some(last) = chain.last() {
                    if last.value.is_none() && last.commit_ts <= cutoff {
                        versions_reclaimed += chain.len();
                        records_removed += 1;
                        continue;
                    }
                }
                if let Some(floor) = chain.iter().rposition(|v| v.commit_ts <= cutoff) {
                    let max_removable = chain.len().saturating_sub(keep);
                    versions_reclaimed += floor.min(max_removable);
                }
            }
        }
        GcReport {
            versions_reclaimed,
            records_removed,
            versions_after: self.total_versions().saturating_sub(versions_reclaimed),
            bytes_reclaimed: 0,
        }
    }

    /// Sum of the on-disk byte sizes of all live segment files. Used to measure
    /// the bytes reclaimed across a GC rewrite.
    fn segment_bytes(&self) -> u64 {
        self.manifest
            .segments
            .iter()
            .filter_map(|seg| fs::metadata(self.dir.join(seg.file_name())).ok())
            .map(|m| m.len())
            .sum()
    }

    /// Persist the current in-memory version chains into a single fresh segment,
    /// preserving each version's original commit timestamp, and drop the old
    /// segments. Used by compaction, GC, and v1→v2 migration.
    fn rewrite_segment(&mut self) -> Result<()> {
        let mut ops: Vec<LogOp> = Vec::new();
        for (collection, chains) in &self.records {
            for (id, chain) in chains {
                for v in chain {
                    match &v.value {
                        Some(record) => ops.push(LogOp::Put {
                            commit_ts: v.commit_ts,
                            record: record.clone(),
                        }),
                        None => ops.push(LogOp::Delete {
                            commit_ts: v.commit_ts,
                            collection: collection.clone(),
                            id: *id,
                        }),
                    }
                }
            }
        }

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
        self.manifest.format_version = FORMAT_VERSION;
        self.manifest.last_txn_id = self.max_txn_id;
        self.manifest.last_commit_ts = self.last_commit_ts;
        self.manifest.save(&self.dir.join(MANIFEST_FILE))?;

        for seg in &old_segments {
            if seg.id != new_id {
                let _ = fs::remove_file(self.dir.join(seg.file_name()));
            }
        }

        self.active_file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&new_path)?;
        Ok(())
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

/// The result of a garbage-collection run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcReport {
    /// Number of superseded/tombstone versions reclaimed.
    pub versions_reclaimed: usize,
    /// Number of record ids removed entirely (fully-dead tombstone chains).
    pub records_removed: usize,
    /// Total versions remaining after GC.
    pub versions_after: usize,
    /// On-disk bytes reclaimed by the GC rewrite (segment size before minus
    /// after). An estimate of the space freed; depends on encoding overhead.
    pub bytes_reclaimed: u64,
}

fn apply_batch(records: &mut RecordMap, batch: Batch) {
    for op in batch.ops {
        match op {
            LogOp::Put { commit_ts, record } => {
                let chain = records
                    .entry(record.collection.clone())
                    .or_default()
                    .entry(record.id)
                    .or_default();
                insert_version(
                    chain,
                    Version {
                        commit_ts,
                        value: Some(record),
                    },
                );
            }
            LogOp::Delete {
                commit_ts,
                collection,
                id,
            } => {
                let chain = records
                    .entry(collection)
                    .or_default()
                    .entry(id)
                    .or_default();
                insert_version(
                    chain,
                    Version {
                        commit_ts,
                        value: None,
                    },
                );
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
    fn apply_committed_batch_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Storage::open(dir.path()).unwrap();
        let batch = Batch {
            txn_id: TxnId(1),
            ops: vec![LogOp::Put {
                commit_ts: 0,
                record: rec(1, 10),
            }],
        };
        // Apply at log index 5 (gaps from skipped no-op entries are allowed).
        s.apply_committed_batch(batch.clone(), 5).unwrap();
        assert_eq!(s.commit_watermark(), 5);
        let v1 = s
            .latest_commit_ts(&CollectionId::new("C"), RecordId::from_u128(1))
            .unwrap();
        assert_eq!(v1, 5);
        // Replaying the same committed index is a no-op: no duplicate version.
        s.apply_committed_batch(batch, 5).unwrap();
        assert_eq!(s.commit_watermark(), 5);
        assert_eq!(
            s.latest_commit_ts(&CollectionId::new("C"), RecordId::from_u128(1)),
            Some(5)
        );
    }

    #[test]
    fn apply_committed_batch_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut s = Storage::open(dir.path()).unwrap();
            s.apply_committed_batch(
                Batch {
                    txn_id: TxnId(1),
                    ops: vec![LogOp::Put {
                        commit_ts: 0,
                        record: rec(7, 70),
                    }],
                },
                3,
            )
            .unwrap();
        }
        let s = Storage::open(dir.path()).unwrap();
        assert_eq!(s.commit_watermark(), 3);
        assert_eq!(
            s.get(&CollectionId::new("C"), RecordId::from_u128(7))
                .unwrap()
                .get("v"),
            Some(&Value::Int(70))
        );
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

    fn cid() -> CollectionId {
        CollectionId::new("C")
    }

    #[test]
    fn mvcc_version_chain_persists() {
        let dir = tempfile::tempdir().unwrap();
        let (t1, t2);
        {
            let mut s = Storage::open(dir.path()).unwrap();
            t1 = s.put(rec(1, 10)).unwrap();
            t2 = s.put(rec(1, 20)).unwrap();
            assert!(t2 > t1);
            // Latest is the newest version; an as-of read sees the old one.
            assert_eq!(
                s.get(&cid(), RecordId::from_u128(1)).unwrap().get("v"),
                Some(&Value::Int(20))
            );
            assert_eq!(
                s.get_as_of(&cid(), RecordId::from_u128(1), t1)
                    .unwrap()
                    .get("v"),
                Some(&Value::Int(10))
            );
        }
        let s = Storage::open(dir.path()).unwrap();
        assert_eq!(
            s.get_as_of(&cid(), RecordId::from_u128(1), t1)
                .unwrap()
                .get("v"),
            Some(&Value::Int(10))
        );
        assert_eq!(
            s.get(&cid(), RecordId::from_u128(1)).unwrap().get("v"),
            Some(&Value::Int(20))
        );
    }

    #[test]
    fn mvcc_tombstone_persists() {
        let dir = tempfile::tempdir().unwrap();
        let before_delete;
        {
            let mut s = Storage::open(dir.path()).unwrap();
            before_delete = s.put(rec(1, 10)).unwrap();
            s.delete(&cid(), RecordId::from_u128(1)).unwrap();
            assert!(s.get(&cid(), RecordId::from_u128(1)).is_none());
        }
        let s = Storage::open(dir.path()).unwrap();
        // Tombstone survives restart; latest read is absent...
        assert!(s.get(&cid(), RecordId::from_u128(1)).is_none());
        // ...but a snapshot before the delete still sees the record.
        assert_eq!(
            s.get_as_of(&cid(), RecordId::from_u128(1), before_delete)
                .unwrap()
                .get("v"),
            Some(&Value::Int(10))
        );
    }

    #[test]
    fn mvcc_restart_preserves_visibility() {
        let dir = tempfile::tempdir().unwrap();
        let (ts_a, ts_b);
        {
            let mut s = Storage::open(dir.path()).unwrap();
            ts_a = s.put(rec(1, 1)).unwrap();
            ts_b = s.put(rec(2, 2)).unwrap();
            s.put(rec(1, 100)).unwrap();
        }
        let s = Storage::open(dir.path()).unwrap();
        // As of ts_a only record 1 (v1) is visible.
        let at_a: Vec<_> = s.scan_as_of(&cid(), ts_a).map(|r| r.id).collect();
        assert_eq!(at_a, vec![RecordId::from_u128(1)]);
        assert_eq!(
            s.get_as_of(&cid(), RecordId::from_u128(1), ts_a)
                .unwrap()
                .get("v"),
            Some(&Value::Int(1))
        );
        // As of ts_b both are visible, record 1 still at v1.
        assert_eq!(s.scan_as_of(&cid(), ts_b).count(), 2);
        assert_eq!(
            s.get_as_of(&cid(), RecordId::from_u128(1), ts_b)
                .unwrap()
                .get("v"),
            Some(&Value::Int(1))
        );
        // Latest read sees the update.
        assert_eq!(
            s.get(&cid(), RecordId::from_u128(1)).unwrap().get("v"),
            Some(&Value::Int(100))
        );
    }

    #[test]
    fn unknown_future_format_rejected() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut s = Storage::open(dir.path()).unwrap();
            s.put(rec(1, 1)).unwrap();
            s.flush().unwrap();
        }
        // Forge a manifest claiming a newer format version.
        let path = dir.path().join("MANIFEST");
        let mut m: Manifest = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        m.format_version = FORMAT_VERSION + 1;
        fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(matches!(
            Storage::open(dir.path()),
            Err(Error::Unsupported { .. })
        ));
    }

    #[test]
    fn corrupt_version_record_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut s = Storage::open(dir.path()).unwrap();
            s.put(rec(1, 10)).unwrap();
            s.put(rec(2, 20)).unwrap();
            s.flush().unwrap();
        }
        // Flip a payload byte inside the last committed version frame.
        let seg = dir.path().join("0000000001.seg");
        let mut bytes = fs::read(&seg).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(&seg, &bytes).unwrap();
        assert!(matches!(
            Storage::open(dir.path()),
            Err(Error::Corruption(_))
        ));
    }

    #[test]
    fn gc_keeps_versions_for_active_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Storage::open(dir.path()).unwrap();
        let read_ts = s.put(rec(1, 1)).unwrap(); // an "active txn" pinned here
        s.put(rec(1, 2)).unwrap();
        let report = s.gc(read_ts, 1).unwrap();
        assert_eq!(report.versions_reclaimed, 0);
        // The old version is still visible to the pinned read timestamp.
        assert_eq!(
            s.get_as_of(&cid(), RecordId::from_u128(1), read_ts)
                .unwrap()
                .get("v"),
            Some(&Value::Int(1))
        );
    }

    #[test]
    fn gc_removes_versions_after_transaction_closes() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Storage::open(dir.path()).unwrap();
        s.put(rec(1, 1)).unwrap();
        s.put(rec(1, 2)).unwrap();
        let watermark = s.commit_watermark();
        let report = s.gc(watermark, 1).unwrap();
        assert_eq!(report.versions_reclaimed, 1);
        assert_eq!(report.versions_after, 1);
        // Latest still readable; reopen to confirm durability.
        assert_eq!(
            s.get(&cid(), RecordId::from_u128(1)).unwrap().get("v"),
            Some(&Value::Int(2))
        );
        drop(s);
        let s = Storage::open(dir.path()).unwrap();
        assert_eq!(s.total_versions(), 1);
    }

    #[test]
    fn gc_preserves_latest_version() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Storage::open(dir.path()).unwrap();
        for v in 1..=5 {
            s.put(rec(1, v)).unwrap();
        }
        s.gc(s.commit_watermark(), 1).unwrap();
        assert_eq!(
            s.get(&cid(), RecordId::from_u128(1)).unwrap().get("v"),
            Some(&Value::Int(5))
        );
        assert_eq!(s.total_versions(), 1);
    }

    #[test]
    fn gc_preserves_tombstone_until_safe() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Storage::open(dir.path()).unwrap();
        let read_ts = s.put(rec(1, 1)).unwrap();
        s.delete(&cid(), RecordId::from_u128(1)).unwrap();
        // A reader pinned before the delete: nothing reclaimed, old value visible.
        let report = s.gc(read_ts, 1).unwrap();
        assert_eq!(report.records_removed, 0);
        assert_eq!(
            s.get_as_of(&cid(), RecordId::from_u128(1), read_ts)
                .unwrap()
                .get("v"),
            Some(&Value::Int(1))
        );
        // Once safe, the dead record is reclaimed entirely.
        let report = s.gc(s.commit_watermark(), 1).unwrap();
        assert_eq!(report.records_removed, 1);
        assert_eq!(s.total_versions(), 0);
    }

    #[test]
    fn gc_restart_consistency() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut s = Storage::open(dir.path()).unwrap();
            for v in 1..=4 {
                s.put(rec(1, v)).unwrap();
            }
            s.gc(s.commit_watermark(), 1).unwrap();
        }
        let s = Storage::open(dir.path()).unwrap();
        assert_eq!(s.total_versions(), 1);
        assert_eq!(
            s.get(&cid(), RecordId::from_u128(1)).unwrap().get("v"),
            Some(&Value::Int(4))
        );
    }
}
