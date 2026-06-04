//! The AuraDB engine: storage, indexes, transactions, and query execution
//! composed behind a single synchronous, thread-safe API.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use auradb_core::{
    Cardinality, CollectionId, CollectionSchema, Document, Error, LogicalClock, OnDelete, Record,
    RecordId, Result, TxnId, Value,
};
use auradb_index::CollectionIndexes;
use auradb_query::exec::DataSource;
use auradb_query::{
    self as query, CountQuery, ExistsQuery, ExplainPlan, FindQuery, MigrationEstimate, Mutation,
    MutationResult, Row,
};
use auradb_storage::{Batch, LogOp, Storage, StorageOptions};
use auradb_txn::{Key, StagedOp, Transaction};

use crate::idgen::record_id_for;

/// Engine configuration.
#[derive(Debug, Clone, Default)]
pub struct EngineOptions {
    /// Storage durability options.
    pub storage: StorageOptions,
}

/// Aggregate engine statistics (for observability and health).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineStats {
    /// Number of registered collections.
    pub collections: usize,
    /// Total live records across all collections.
    pub records: usize,
    /// Schema catalog version.
    pub schema_version: u64,
}

/// How collection indexes were initialized when the engine opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexLoadReport {
    /// Collections whose indexes were loaded from a valid persisted snapshot.
    pub loaded: usize,
    /// Collections whose indexes were rebuilt from storage (snapshot absent,
    /// stale, corrupt, or schema-incompatible).
    pub rebuilt: usize,
}

struct Inner {
    storage: Storage,
    indexes: HashMap<String, CollectionIndexes>,
    clock: LogicalClock,
    index_dir: PathBuf,
    load_report: IndexLoadReport,
}

/// The embeddable AuraDB engine. Cheap to clone (shares one locked core).
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Mutex<Inner>>,
}

impl DataSource for Inner {
    fn schema(&self, collection: &str) -> Option<&CollectionSchema> {
        self.storage.get_schema(collection)
    }
    fn indexes(&self, collection: &str) -> Option<&CollectionIndexes> {
        self.indexes.get(collection)
    }
    fn scan<'a>(&'a self, collection: &str) -> Box<dyn Iterator<Item = &'a Record> + 'a> {
        Box::new(
            self.storage
                .scan(&CollectionId::new(collection.to_string())),
        )
    }
    fn get(&self, collection: &str, id: RecordId) -> Option<&Record> {
        self.storage
            .get(&CollectionId::new(collection.to_string()), id)
    }
    fn resolve_link(&self, target: &str, key: &Value) -> Option<&Record> {
        let id = crate::idgen::derive_id(target, key);
        self.storage.get(&CollectionId::new(target.to_string()), id)
    }
}

/// Iterate a collection's records *as seen within a transaction*: committed
/// records overlaid with the transaction's staged puts, with staged deletes and
/// put-shadowed committed records removed. This is the primitive that gives
/// transactional reads read-your-writes semantics.
fn overlay_scan<'a>(
    inner: &'a Inner,
    txn: &'a Transaction,
    collection: &str,
) -> impl Iterator<Item = &'a Record> + 'a {
    let cid = CollectionId::new(collection.to_string());
    let scan_cid = cid.clone();
    // Committed records, minus any the transaction has staged (a staged put
    // shadows the committed version; a staged delete removes it).
    let committed = inner
        .storage
        .scan(&scan_cid)
        .filter(move |r| txn.staged(&Key::new(scan_cid.clone(), r.id)).is_none());
    // The transaction's staged puts for this collection.
    let staged = txn.staged_ops().filter_map(move |(_key, op)| match op {
        StagedOp::Put(record) if record.collection == cid => Some(record),
        StagedOp::Put(_) | StagedOp::Delete => None,
    });
    committed.chain(staged)
}

/// A read-only view of the engine's data *through a transaction*: committed
/// state overlaid with that transaction's staged writes and deletes.
///
/// The view owns a freshly-built overlay [`CollectionIndexes`] for the queried
/// collection so that index-seeded candidate selection (equality lookup, vector
/// nearest, full-text) reflects staged writes. Correctness comes before
/// performance: the overlay index is rebuilt per query (see `docs/TRANSACTIONS.md`).
struct TxnView<'a> {
    inner: &'a Inner,
    txn: &'a Transaction,
    /// Overlay indexes keyed by collection, built over the transaction view so
    /// candidate selection is consistent with [`TxnView::scan`]/[`TxnView::get`].
    overlay: HashMap<String, CollectionIndexes>,
}

impl DataSource for TxnView<'_> {
    fn schema(&self, collection: &str) -> Option<&CollectionSchema> {
        self.inner.storage.get_schema(collection)
    }

    fn indexes(&self, collection: &str) -> Option<&CollectionIndexes> {
        // Prefer the transaction-overlay index when one was built for this
        // collection; otherwise the committed index is identical to the view
        // (no staged ops affect it) and is safe to reuse.
        self.overlay
            .get(collection)
            .or_else(|| self.inner.indexes.get(collection))
    }

    fn scan<'b>(&'b self, collection: &str) -> Box<dyn Iterator<Item = &'b Record> + 'b> {
        Box::new(overlay_scan(self.inner, self.txn, collection))
    }

    fn get(&self, collection: &str, id: RecordId) -> Option<&Record> {
        let key = Key::new(CollectionId::new(collection.to_string()), id);
        match self.txn.staged(&key) {
            Some(StagedOp::Put(record)) => Some(record),
            Some(StagedOp::Delete) => None,
            None => self
                .inner
                .storage
                .get(&CollectionId::new(collection.to_string()), id),
        }
    }

    fn resolve_link(&self, target: &str, key: &Value) -> Option<&Record> {
        let id = crate::idgen::derive_id(target, key);
        self.get(target, id)
    }
}

impl Engine {
    /// Open (creating if necessary) the database at `dir` with default options.
    pub fn open(dir: impl AsRef<Path>) -> Result<Engine> {
        Engine::open_with(dir, EngineOptions::default())
    }

    /// Open (creating if necessary) the database at `dir`.
    pub fn open_with(dir: impl AsRef<Path>, options: EngineOptions) -> Result<Engine> {
        let dir = dir.as_ref().to_path_buf();
        let storage = Storage::open_with(&dir, options.storage)?;
        let clock = LogicalClock::new(storage.max_txn_id() + 1);
        let index_dir = dir.join("indexes");

        // For each collection, load a valid persisted index snapshot if one
        // exists and matches the current storage state; otherwise rebuild from
        // storage. Loading never returns incorrect results: a snapshot is used
        // only when its content fingerprint and schema field shape both match.
        let manifest = auradb_index::persist::load_manifest(&index_dir);
        let mut indexes = HashMap::new();
        let mut load_report = IndexLoadReport::default();
        for schema in storage.list_schemas() {
            let cid = CollectionId::new(schema.name.clone());
            let fingerprint = auradb_index::fingerprint(storage.scan(&cid));
            let loaded = manifest
                .as_ref()
                .and_then(|m| m.files.get(&schema.name))
                .map(|f| index_dir.join(f))
                .and_then(|path| auradb_index::persist::read_snapshot(&path).ok())
                .filter(|snap| snap.fingerprint == fingerprint)
                .and_then(|snap| CollectionIndexes::from_snapshot(schema, snap).ok());
            let idx = match loaded {
                Some(idx) => {
                    load_report.loaded += 1;
                    idx
                }
                None => {
                    let mut idx = CollectionIndexes::from_schema(schema);
                    idx.rebuild(storage.scan(&cid))?;
                    load_report.rebuilt += 1;
                    idx
                }
            };
            indexes.insert(schema.name.clone(), idx);
        }

        Ok(Engine {
            inner: Arc::new(Mutex::new(Inner {
                storage,
                indexes,
                clock,
                index_dir,
                load_report,
            })),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("engine mutex poisoned")
    }

    // ----- schema -----

    /// Register or replace a collection schema, rebuilding its indexes.
    pub fn create_schema(&self, schema: CollectionSchema) -> Result<()> {
        let mut inner = self.lock();
        schema.validate_definition()?;
        let name = schema.name.clone();
        inner.storage.put_schema(schema.clone())?;
        let mut idx = CollectionIndexes::from_schema(&schema);
        idx.rebuild(inner.storage.scan(&CollectionId::new(name.clone())))?;
        inner.indexes.insert(name, idx);
        Ok(())
    }

    /// Drop a collection schema and all of its records.
    pub fn drop_schema(&self, name: &str) -> Result<()> {
        let mut inner = self.lock();
        inner.storage.drop_schema(name)?;
        inner.indexes.remove(name);
        Ok(())
    }

    /// Fetch a schema by name.
    pub fn get_schema(&self, name: &str) -> Option<CollectionSchema> {
        self.lock().storage.get_schema(name).cloned()
    }

    /// List all registered schemas.
    pub fn list_schemas(&self) -> Vec<CollectionSchema> {
        self.lock()
            .storage
            .list_schemas()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Estimate the impact of migrating a collection to `target`.
    pub fn migration_estimate(&self, target: &CollectionSchema) -> Result<MigrationEstimate> {
        let inner = self.lock();
        query::estimate_migration(&*inner, target)
    }

    // ----- reads -----

    /// Plan a find and return ordered ids/scores plus the EXPLAIN plan.
    pub fn plan_find(&self, q: &FindQuery) -> Result<query::PlannedFind> {
        let inner = self.lock();
        query::execute_find(&*inner, q)
    }

    /// Materialize specific rows of a find (used for cursor paging).
    pub fn materialize(&self, q: &FindQuery, page: &[(RecordId, Option<f32>)]) -> Result<Vec<Row>> {
        let inner = self.lock();
        query::materialize(&*inner, q, page)
    }

    /// Run a find to completion, returning all matching rows.
    pub fn find(&self, q: &FindQuery) -> Result<Vec<Row>> {
        let inner = self.lock();
        let planned = query::execute_find(&*inner, q)?;
        query::materialize(&*inner, q, &planned.ordered)
    }

    /// Count matching records.
    pub fn count(&self, q: &CountQuery) -> Result<usize> {
        let inner = self.lock();
        query::execute_count(&*inner, q)
    }

    /// Test whether any record matches.
    pub fn exists(&self, q: &ExistsQuery) -> Result<bool> {
        let inner = self.lock();
        query::execute_exists(&*inner, q)
    }

    /// Produce an EXPLAIN plan for a find.
    pub fn explain(&self, q: &FindQuery) -> Result<ExplainPlan> {
        let inner = self.lock();
        query::explain(&*inner, q)
    }

    // ----- auto-commit mutations -----

    /// Apply a mutation in auto-commit mode.
    pub fn apply_mutation(&self, mutation: Mutation) -> Result<MutationResult> {
        let mut inner = self.lock();
        let txn_id = TxnId::AUTO;
        match mutation {
            Mutation::Insert { collection, fields } => {
                inner.write_records(&collection, vec![fields], WriteMode::Insert, txn_id)
            }
            Mutation::Upsert { collection, fields } => {
                inner.write_records(&collection, vec![fields], WriteMode::Upsert, txn_id)
            }
            Mutation::BulkInsert {
                collection,
                records,
            } => inner.write_records(&collection, records, WriteMode::Insert, txn_id),
            Mutation::Update {
                collection,
                filter,
                set,
            } => inner.update_records(&collection, filter.as_ref(), &set, txn_id),
            Mutation::Delete { collection, filter } => {
                inner.delete_records(&collection, filter.as_ref(), txn_id)
            }
        }
    }

    /// Convenience: insert one record, returning its id.
    pub fn insert(&self, collection: &str, fields: Document) -> Result<RecordId> {
        let result = self.apply_mutation(Mutation::Insert {
            collection: collection.to_string(),
            fields,
        })?;
        result
            .ids
            .first()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::Internal("insert produced no id".into()))
    }

    // ----- transactions -----

    /// Begin a transaction, returning its id.
    pub fn begin(&self) -> Transaction {
        let inner = self.lock();
        Transaction::begin(TxnId(inner.clock.tick()))
    }

    /// Stage a mutation within a transaction (no durable write yet).
    pub fn stage(&self, txn: &mut Transaction, mutation: Mutation) -> Result<MutationResult> {
        let inner = self.lock();
        inner.stage_mutation(txn, mutation)
    }

    /// Read a record within a transaction, honoring staged writes
    /// (read-your-writes).
    pub fn txn_get(&self, txn: &Transaction, collection: &str, id: RecordId) -> Option<Record> {
        let key = Key::new(CollectionId::new(collection.to_string()), id);
        match txn.staged(&key) {
            Some(StagedOp::Put(r)) => Some(r.clone()),
            Some(StagedOp::Delete) => None,
            None => self.lock().get(collection, id).cloned(),
        }
    }

    // ----- transaction-scoped reads -----
    //
    // Every read below executes against a [`TxnView`]: the committed state
    // overlaid with the transaction's own staged writes and deletes. This gives
    // read-your-writes (a transaction sees its staged inserts/updates and does
    // not see its staged deletes) while leaving non-transactional reads, which
    // never construct a `TxnView`, exactly as they were.

    /// Plan a find within a transaction. Candidate selection (index lookup,
    /// vector, full-text) runs against the transaction view, so staged writes
    /// are visible and staged deletes are hidden.
    pub fn txn_plan_find(&self, txn: &Transaction, q: &FindQuery) -> Result<query::PlannedFind> {
        let inner = self.lock();
        let view = inner.txn_view(txn, &q.collection)?;
        query::execute_find(&view, q)
    }

    /// Materialize specific rows of a transactional find (used for cursor
    /// paging within a transaction).
    pub fn txn_materialize(
        &self,
        txn: &Transaction,
        q: &FindQuery,
        page: &[(RecordId, Option<f32>)],
    ) -> Result<Vec<Row>> {
        let inner = self.lock();
        let view = inner.txn_view(txn, &q.collection)?;
        query::materialize(&view, q, page)
    }

    /// Run a find to completion within a transaction, returning all matching
    /// rows from the transaction view.
    pub fn txn_find(&self, txn: &Transaction, q: &FindQuery) -> Result<Vec<Row>> {
        let inner = self.lock();
        let view = inner.txn_view(txn, &q.collection)?;
        let planned = query::execute_find(&view, q)?;
        query::materialize(&view, q, &planned.ordered)
    }

    /// Count matching records within a transaction.
    pub fn txn_count(&self, txn: &Transaction, q: &CountQuery) -> Result<usize> {
        let inner = self.lock();
        let view = inner.txn_view(txn, &q.collection)?;
        query::execute_count(&view, q)
    }

    /// Test whether any record matches within a transaction.
    pub fn txn_exists(&self, txn: &Transaction, q: &ExistsQuery) -> Result<bool> {
        let inner = self.lock();
        let view = inner.txn_view(txn, &q.collection)?;
        query::execute_exists(&view, q)
    }

    /// Produce an EXPLAIN plan for a find within a transaction.
    pub fn txn_explain(&self, txn: &Transaction, q: &FindQuery) -> Result<ExplainPlan> {
        let inner = self.lock();
        let view = inner.txn_view(txn, &q.collection)?;
        query::explain(&view, q)
    }

    /// Commit a transaction atomically, with optimistic conflict detection.
    pub fn commit(&self, txn: Transaction) -> Result<()> {
        let mut inner = self.lock();
        inner.commit_transaction(txn)
    }

    /// Roll back a transaction, discarding all staged writes.
    pub fn rollback(&self, mut txn: Transaction) {
        txn.finish();
        drop(txn);
    }

    // ----- maintenance -----

    /// Compact storage, preserving all live data, then refresh persisted index
    /// snapshots so a subsequent open loads them directly.
    pub fn compact(&self) -> Result<auradb_storage::CompactionReport> {
        let mut inner = self.lock();
        let report = inner.storage.compact()?;
        inner.persist_indexes()?;
        Ok(report)
    }

    /// Flush storage durably.
    pub fn flush(&self) -> Result<()> {
        self.lock().storage.flush()
    }

    /// Flush storage and persist index snapshots: a durable checkpoint after
    /// which a fresh open loads indexes from disk rather than rebuilding them.
    pub fn checkpoint(&self) -> Result<()> {
        let mut inner = self.lock();
        inner.storage.flush()?;
        inner.persist_indexes()
    }

    /// Persist index snapshots without flushing storage.
    pub fn persist_indexes(&self) -> Result<()> {
        self.lock().persist_indexes()
    }

    /// How indexes were initialized on open (loaded from disk vs rebuilt).
    pub fn index_load_report(&self) -> IndexLoadReport {
        self.lock().load_report.clone()
    }

    /// Rebuild every index from storage and persist fresh snapshots. Used by
    /// `auradb index rebuild` and as a recovery path.
    pub fn rebuild_indexes(&self) -> Result<IndexLoadReport> {
        let mut inner = self.lock();
        let schemas: Vec<CollectionSchema> =
            inner.storage.list_schemas().into_iter().cloned().collect();
        inner.indexes.clear();
        for schema in &schemas {
            let cid = CollectionId::new(schema.name.clone());
            let mut idx = CollectionIndexes::from_schema(schema);
            idx.rebuild(inner.storage.scan(&cid))?;
            inner.indexes.insert(schema.name.clone(), idx);
        }
        inner.load_report = IndexLoadReport {
            loaded: 0,
            rebuilt: schemas.len(),
        };
        inner.persist_indexes()?;
        Ok(inner.load_report.clone())
    }

    /// Verify that every index is consistent with stored records.
    pub fn check_consistency(&self) -> Result<usize> {
        let inner = self.lock();
        let mut total = 0;
        for (name, idx) in &inner.indexes {
            total += idx.consistency_check(inner.storage.scan(&CollectionId::new(name.clone())))?;
        }
        Ok(total)
    }

    /// Current aggregate statistics.
    pub fn stats(&self) -> EngineStats {
        let inner = self.lock();
        EngineStats {
            collections: inner.storage.collection_count(),
            records: inner.storage.total_records(),
            schema_version: inner.storage.schema_version(),
        }
    }
}

/// Whether a write must be a fresh insert or may replace an existing record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteMode {
    Insert,
    Upsert,
}

impl Inner {
    /// Write a persisted snapshot for every collection index plus an updated
    /// manifest, and prune snapshot files for dropped collections.
    fn persist_indexes(&self) -> Result<()> {
        use auradb_index::persist;

        std::fs::create_dir_all(&self.index_dir)?;
        let schema_version = self.storage.schema_version();
        let mut files = BTreeMap::new();
        for (name, idx) in &self.indexes {
            let cid = CollectionId::new(name.clone());
            let fingerprint = auradb_index::fingerprint(self.storage.scan(&cid));
            let snapshot = idx.snapshot(schema_version, fingerprint);
            let file = persist::index_filename(name);
            persist::write_snapshot(&self.index_dir.join(&file), &snapshot)?;
            files.insert(name.clone(), file);
        }

        // Remove snapshot files for collections that no longer exist.
        let keep: HashSet<String> = files.values().cloned().collect();
        if let Ok(entries) = std::fs::read_dir(&self.index_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.ends_with(".idx") && !keep.contains(name.as_ref()) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }

        persist::save_manifest(
            &self.index_dir,
            &persist::IndexManifest {
                format_version: persist::INDEX_FORMAT_VERSION,
                files,
            },
        )?;
        Ok(())
    }

    fn schema_for(&self, collection: &str) -> Result<CollectionSchema> {
        self.storage
            .get_schema(collection)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("collection {collection}")))
    }

    /// Verify that every relationship value references an existing target.
    fn referential_check(&self, schema: &CollectionSchema, fields: &Document) -> Result<()> {
        for rel in &schema.relationships {
            let Some(value) = fields.get(&rel.name) else {
                continue;
            };
            let keys: Vec<&Value> = match (rel.cardinality, value) {
                (_, Value::Null) => Vec::new(),
                (Cardinality::ToOne, v) => vec![v],
                (Cardinality::ToMany, Value::Array(items)) => items.iter().collect(),
                _ => {
                    return Err(Error::SchemaViolation(format!(
                        "relationship {} has the wrong shape for its cardinality",
                        rel.name
                    )))
                }
            };
            for key in keys {
                if self.resolve_link(&rel.target, key).is_none() {
                    return Err(Error::SchemaViolation(format!(
                        "relationship {} references a missing {} record",
                        rel.name, rel.target
                    )));
                }
            }
        }
        Ok(())
    }

    /// Reject deleting `target` from `collection` if a Restrict relationship
    /// points at its primary-key value.
    fn inbound_link_check(&self, collection: &str, target: &Record) -> Result<()> {
        // The primary-key value other records reference this one by.
        let Some(schema) = self.storage.get_schema(collection) else {
            return Ok(());
        };
        let Some(pk) = schema.primary_key() else {
            return Ok(());
        };
        let Some(pk_value) = target.fields.get(&pk.name) else {
            return Ok(());
        };
        for ref_schema in self.storage.list_schemas() {
            for rel in &ref_schema.relationships {
                if rel.target != collection || rel.on_delete != OnDelete::Restrict {
                    continue;
                }
                for record in self
                    .storage
                    .scan(&CollectionId::new(ref_schema.name.clone()))
                {
                    let refers = match record.get(&rel.name) {
                        Some(v @ Value::Text(_)) => v == pk_value,
                        Some(Value::Array(items)) => items.iter().any(|v| v == pk_value),
                        _ => false,
                    };
                    if refers {
                        return Err(Error::Conflict(format!(
                            "cannot delete {collection} record: referenced by {} via {}",
                            ref_schema.name, rel.name
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    fn next_id(&self) -> u64 {
        self.clock.tick()
    }

    /// Insert or upsert a batch of records atomically (auto-commit).
    fn write_records(
        &mut self,
        collection: &str,
        list: Vec<Document>,
        mode: WriteMode,
        txn_id: TxnId,
    ) -> Result<MutationResult> {
        let schema = self.schema_for(collection)?;
        let cid = CollectionId::new(collection.to_string());
        let mut prepared: Vec<(Option<Record>, Record)> = Vec::new();
        // Track uniqueness within this batch (field + value key -> nothing).
        let mut pending_unique: HashSet<String> = HashSet::new();

        for fields in list {
            schema.validate_record(&fields)?;
            let id = record_id_for(&schema, &fields, self.next_id())?;
            let existing = self.storage.get(&cid, id).cloned();
            if existing.is_some() && mode == WriteMode::Insert {
                return Err(Error::UniqueViolation(format!(
                    "{collection} record with this primary key already exists"
                )));
            }
            self.referential_check(&schema, &fields)?;
            let version = existing.as_ref().map(|r| r.version + 1).unwrap_or(1);
            let record = Record {
                id,
                collection: cid.clone(),
                fields,
                version,
                created_txn: txn_id,
            };
            let idx = &self.indexes[collection];
            idx.check_unique(&record, existing.as_ref().map(|r| r.id))?;
            self.check_pending_unique(&schema, &record, &mut pending_unique)?;
            prepared.push((existing, record));
        }

        let ops = prepared
            .iter()
            .map(|(_, r)| LogOp::Put { record: r.clone() })
            .collect();
        self.storage.commit_batch(Batch { txn_id, ops })?;

        let idx = self.indexes.get_mut(collection).expect("indexes exist");
        let mut result = MutationResult::empty();
        for (old, new) in prepared {
            if let Some(old) = &old {
                idx.remove(old);
            }
            idx.insert(&new);
            result.ids.push(new.id.to_string());
            if old.is_some() {
                result.updated += 1;
            } else {
                result.inserted += 1;
            }
        }
        Ok(result)
    }

    fn check_pending_unique(
        &self,
        schema: &CollectionSchema,
        record: &Record,
        pending: &mut HashSet<String>,
    ) -> Result<()> {
        for field in &schema.fields {
            if !(field.primary_key || field.unique) {
                continue;
            }
            if let Some(value) = record.fields.get(&field.name) {
                if value.is_null() {
                    continue;
                }
                let key = format!(
                    "{}\u{0}{}",
                    field.name,
                    serde_json::to_string(&value.to_json()).unwrap_or_default()
                );
                if !pending.insert(key) {
                    return Err(Error::UniqueViolation(format!(
                        "duplicate value for unique field {} within batch",
                        field.name
                    )));
                }
            }
        }
        Ok(())
    }

    fn update_records(
        &mut self,
        collection: &str,
        filter: Option<&query::Filter>,
        set: &Document,
        txn_id: TxnId,
    ) -> Result<MutationResult> {
        let schema = self.schema_for(collection)?;
        let cid = CollectionId::new(collection.to_string());

        // Disallow changing the primary key via update.
        if let Some(pk) = schema.primary_key() {
            if set.contains_key(&pk.name) {
                return Err(Error::InvalidRequest(
                    "cannot change the primary key in an update".into(),
                ));
            }
        }

        let targets: Vec<Record> = self
            .storage
            .scan(&cid)
            .filter(|r| filter.map(|f| query::eval::matches(r, f)).unwrap_or(true))
            .cloned()
            .collect();

        let mut prepared: Vec<(Record, Record)> = Vec::new();
        for old in targets {
            let mut fields = old.fields.clone();
            for (k, v) in set {
                fields.insert(k.clone(), v.clone());
            }
            schema.validate_record(&fields)?;
            self.referential_check(&schema, &fields)?;
            let new = Record {
                id: old.id,
                collection: cid.clone(),
                fields,
                version: old.version + 1,
                created_txn: txn_id,
            };
            self.indexes[collection].check_unique(&new, Some(old.id))?;
            prepared.push((old, new));
        }

        if prepared.is_empty() {
            return Ok(MutationResult::empty());
        }
        let ops = prepared
            .iter()
            .map(|(_, r)| LogOp::Put { record: r.clone() })
            .collect();
        self.storage.commit_batch(Batch { txn_id, ops })?;

        let idx = self.indexes.get_mut(collection).expect("indexes exist");
        let mut result = MutationResult::empty();
        for (old, new) in prepared {
            idx.remove(&old);
            idx.insert(&new);
            result.updated += 1;
            result.ids.push(new.id.to_string());
        }
        Ok(result)
    }

    fn delete_records(
        &mut self,
        collection: &str,
        filter: Option<&query::Filter>,
        txn_id: TxnId,
    ) -> Result<MutationResult> {
        let _ = self.schema_for(collection)?;
        let cid = CollectionId::new(collection.to_string());
        let targets: Vec<Record> = self
            .storage
            .scan(&cid)
            .filter(|r| filter.map(|f| query::eval::matches(r, f)).unwrap_or(true))
            .cloned()
            .collect();

        for old in &targets {
            self.inbound_link_check(collection, old)?;
        }
        if targets.is_empty() {
            return Ok(MutationResult::empty());
        }

        let ops = targets
            .iter()
            .map(|r| LogOp::Delete {
                collection: cid.clone(),
                id: r.id,
            })
            .collect();
        self.storage.commit_batch(Batch { txn_id, ops })?;

        let idx = self.indexes.get_mut(collection).expect("indexes exist");
        let mut result = MutationResult::empty();
        for old in targets {
            idx.remove(&old);
            result.deleted += 1;
            result.ids.push(old.id.to_string());
        }
        Ok(result)
    }

    // ----- transactional reads -----

    /// Build a [`TxnView`] for reading within `txn`. An overlay index is built
    /// for `collection` (the query target) so index-seeded selection reflects
    /// staged writes; other collections fall back to their committed indexes,
    /// which are unaffected because relationship hydration uses `get`, not an
    /// index seed.
    fn txn_view<'a>(&'a self, txn: &'a Transaction, collection: &str) -> Result<TxnView<'a>> {
        let mut overlay = HashMap::new();
        // Only the queried collection needs an overlay index, and only when the
        // transaction has staged something at all (otherwise the view equals the
        // committed state and the committed index is exact).
        if !txn.is_empty() {
            if let Some(schema) = self.storage.get_schema(collection) {
                let mut idx = CollectionIndexes::from_schema(schema);
                idx.rebuild(overlay_scan(self, txn, collection))?;
                overlay.insert(collection.to_string(), idx);
            }
        }
        Ok(TxnView {
            inner: self,
            txn,
            overlay,
        })
    }

    // ----- transactional staging -----

    fn stage_mutation(&self, txn: &mut Transaction, mutation: Mutation) -> Result<MutationResult> {
        let collection = mutation.collection().to_string();
        let schema = self.schema_for(&collection)?;
        let cid = CollectionId::new(collection.clone());
        let mut result = MutationResult::empty();

        match mutation {
            Mutation::Insert { fields, .. } | Mutation::Upsert { fields, .. } => {
                schema.validate_record(&fields)?;
                let id = record_id_for(&schema, &fields, self.clock.tick())?;
                let existing = self.storage.get(&cid, id).cloned();
                self.referential_check(&schema, &fields)?;
                let record = Record {
                    id,
                    collection: cid.clone(),
                    fields,
                    version: 0, // assigned at commit
                    created_txn: txn.id(),
                };
                txn.stage_put(record, existing.as_ref().map(|r| r.version));
                if existing.is_some() {
                    result.updated += 1;
                } else {
                    result.inserted += 1;
                }
                result.ids.push(id.to_string());
            }
            Mutation::BulkInsert { records, .. } => {
                for fields in records {
                    schema.validate_record(&fields)?;
                    let id = record_id_for(&schema, &fields, self.clock.tick())?;
                    let existing = self.storage.get(&cid, id).cloned();
                    self.referential_check(&schema, &fields)?;
                    let record = Record {
                        id,
                        collection: cid.clone(),
                        fields,
                        version: 0,
                        created_txn: txn.id(),
                    };
                    txn.stage_put(record, existing.as_ref().map(|r| r.version));
                    result.inserted += 1;
                    result.ids.push(id.to_string());
                }
            }
            Mutation::Update { filter, set, .. } => {
                let targets: Vec<Record> = self
                    .storage
                    .scan(&cid)
                    .filter(|r| {
                        filter
                            .as_ref()
                            .map(|f| query::eval::matches(r, f))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect();
                for old in targets {
                    let mut fields = old.fields.clone();
                    for (k, v) in &set {
                        fields.insert(k.clone(), v.clone());
                    }
                    schema.validate_record(&fields)?;
                    self.referential_check(&schema, &fields)?;
                    let record = Record {
                        id: old.id,
                        collection: cid.clone(),
                        fields,
                        version: 0,
                        created_txn: txn.id(),
                    };
                    txn.stage_put(record, Some(old.version));
                    result.updated += 1;
                    result.ids.push(old.id.to_string());
                }
            }
            Mutation::Delete { filter, .. } => {
                let targets: Vec<Record> = self
                    .storage
                    .scan(&cid)
                    .filter(|r| {
                        filter
                            .as_ref()
                            .map(|f| query::eval::matches(r, f))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect();
                for old in targets {
                    txn.stage_delete(cid.clone(), old.id, Some(old.version));
                    result.deleted += 1;
                    result.ids.push(old.id.to_string());
                }
            }
        }
        Ok(result)
    }

    fn commit_transaction(&mut self, txn: Transaction) -> Result<()> {
        let txn_id = txn.id();
        let (observed, staged) = txn.into_parts();

        // 1. Optimistic conflict detection against committed versions.
        for (key, observed_version) in &observed {
            let current = self.storage.get(&key.collection, key.id).map(|r| r.version);
            if current != *observed_version {
                return Err(Error::Conflict(format!(
                    "record {} in {} changed concurrently",
                    key.id, key.collection
                )));
            }
        }

        // 2. Validate uniqueness / referential integrity and assign versions.
        let mut batch_ops: Vec<LogOp> = Vec::new();
        let mut index_updates: Vec<(Option<Record>, Option<Record>)> = Vec::new();
        for (key, op) in staged {
            match op {
                StagedOp::Put(mut record) => {
                    let collection = key.collection.as_str();
                    let schema = self.schema_for(collection)?;
                    let existing = self.storage.get(&key.collection, key.id).cloned();
                    self.referential_check(&schema, &record.fields)?;
                    if let Some(idx) = self.indexes.get(collection) {
                        idx.check_unique(&record, existing.as_ref().map(|r| r.id))?;
                    }
                    record.version = existing.as_ref().map(|r| r.version + 1).unwrap_or(1);
                    record.created_txn = txn_id;
                    batch_ops.push(LogOp::Put {
                        record: record.clone(),
                    });
                    index_updates.push((existing, Some(record)));
                }
                StagedOp::Delete => {
                    let existing = self.storage.get(&key.collection, key.id).cloned();
                    if let Some(existing) = &existing {
                        self.inbound_link_check(key.collection.as_str(), existing)?;
                    }
                    batch_ops.push(LogOp::Delete {
                        collection: key.collection.clone(),
                        id: key.id,
                    });
                    index_updates.push((existing, None));
                }
            }
        }

        if batch_ops.is_empty() {
            return Ok(());
        }

        // 3. Durable atomic commit.
        self.storage.commit_batch(Batch {
            txn_id,
            ops: batch_ops,
        })?;

        // 4. Update indexes.
        for (old, new) in index_updates {
            let collection = old
                .as_ref()
                .or(new.as_ref())
                .map(|r| r.collection.0.clone())
                .expect("at least one side present");
            if let Some(idx) = self.indexes.get_mut(&collection) {
                if let Some(old) = &old {
                    idx.remove(old);
                }
                if let Some(new) = &new {
                    idx.insert(new);
                }
            }
        }
        Ok(())
    }
}
