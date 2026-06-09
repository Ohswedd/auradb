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
    self as query, CollectionStats, CountQuery, ExistsQuery, ExplainPlan, FindQuery,
    MigrationEstimate, Mutation, MutationResult, PlannerStats, Row, Scored,
};
use auradb_storage::{Batch, LogOp, Storage, StorageOptions};
use auradb_txn::{Key, StagedOp, Transaction};

use crate::clock::WallClock;
use crate::idgen::record_id_for;

/// Default transaction idle timeout in seconds (mirrors the server's
/// `[mvcc] transaction_timeout_secs` default).
pub const DEFAULT_TRANSACTION_TIMEOUT_SECS: u64 = 300;

/// Engine configuration.
#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// Storage durability options.
    pub storage: StorageOptions,
    /// Minimum number of most-recent versions of each live record GC retains.
    pub gc_min_retained_versions: usize,
    /// Idle timeout after which an unfinished transaction is reaped: its
    /// snapshot is released and further operations on it are rejected. `0`
    /// disables transaction timeouts.
    pub transaction_timeout_secs: u64,
    /// Wall-clock time source. Defaults to the system clock; tests inject a
    /// manual clock so timeouts are deterministic.
    pub clock: WallClock,
}

impl Default for EngineOptions {
    fn default() -> Self {
        EngineOptions {
            storage: StorageOptions::default(),
            gc_min_retained_versions: 1,
            transaction_timeout_secs: DEFAULT_TRANSACTION_TIMEOUT_SECS,
            clock: WallClock::System,
        }
    }
}

/// A replicated write log the engine routes commits through in cluster mode.
///
/// When a [`ReplicatedLog`] is attached (via [`Engine::attach_replicated_log`]),
/// every data-plane commit is first appended to it and only applied to storage
/// once consensus has committed the entry. The returned value is the committed
/// **log index**, which the engine uses as the MVCC commit timestamp so that
/// every replica derives identical, monotonic timestamps from the same ordered
/// log. When no log is attached (the default, single-node path) commits go
/// straight to storage exactly as in previous releases.
pub trait ReplicatedLog: Send + Sync {
    /// Append `batch` to the replicated log, block until it is committed, and
    /// return the committed log index. Returns [`Error::NotLeader`] if this node
    /// is not the current leader and therefore cannot accept writes.
    fn replicate(&self, batch: &Batch) -> Result<u64>;
}

/// The lifecycle state of a registered transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxnState {
    /// Open and able to read and stage writes.
    Active,
    /// Reaped after exceeding the idle timeout. Its snapshot has been released;
    /// any further operation on it is rejected with a transaction-timeout error.
    TimedOut,
}

/// Per-transaction bookkeeping held by the active-transaction registry. The
/// registry is the single source of truth for which snapshots are pinned, so GC
/// can never reclaim a version a live transaction can still observe and an
/// abandoned transaction can be reaped instead of pinning versions forever.
#[derive(Debug, Clone)]
struct TxnEntry {
    /// The pinned MVCC read timestamp (snapshot).
    read_ts: u64,
    /// Wall-clock second the transaction began.
    started_at: u64,
    /// Wall-clock second of the most recent operation on the transaction.
    last_activity: u64,
    /// The owning connection, when the transaction was begun on the server.
    connection_id: Option<u64>,
    /// Lifecycle state.
    state: TxnState,
}

/// A public, point-in-time view of one registered transaction, for status and
/// observability output. Carries no record data.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActiveTransaction {
    /// The transaction id.
    pub id: u64,
    /// The pinned MVCC read timestamp (snapshot).
    pub read_ts: u64,
    /// Age in seconds since the transaction began.
    pub age_secs: u64,
    /// Seconds since the most recent operation on the transaction.
    pub idle_secs: u64,
    /// The owning connection, if known.
    pub connection_id: Option<u64>,
    /// Lifecycle state.
    pub state: TxnState,
}

/// Aggregate engine statistics (for observability and health).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineStats {
    /// Number of registered collections.
    pub collections: usize,
    /// Total live records across all collections.
    pub records: usize,
    /// Total stored MVCC versions (including superseded versions and tombstones).
    pub versions: usize,
    /// Number of transactions currently holding a pinned snapshot (state Active).
    pub active_transactions: usize,
    /// Number of registered transactions that have timed out but have not yet
    /// been cleaned up by their connection.
    pub timed_out_transactions: usize,
    /// The oldest read timestamp pinned by an active transaction, if any. This
    /// is the GC reclamation horizon when transactions are active.
    pub oldest_active_read_ts: Option<u64>,
    /// Age in seconds of the oldest active transaction, if any.
    pub oldest_transaction_age_secs: Option<u64>,
    /// Cumulative number of transactions reaped for exceeding the idle timeout.
    pub transaction_timeouts_total: u64,
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

/// Search-index summary for one full-text field.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TextIndexInfo {
    /// The indexed field name.
    pub field: String,
    /// Number of indexed documents (BM25 corpus size).
    pub documents: usize,
    /// Number of distinct terms in the inverted index.
    pub distinct_terms: usize,
    /// Average document length in tokens (BM25 `avgdl`).
    pub avg_doc_len: f32,
    /// A warning when statistics look incomplete, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Search-index summary for one vector field, including opt-in HNSW/ANN
/// **preview** lifecycle status. Exact search is always available and is the
/// correctness baseline; these fields describe only the optional preview.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VectorIndexInfo {
    /// The indexed field name.
    pub field: String,
    /// Vector dimensionality.
    pub dim: usize,
    /// Number of indexed vectors.
    pub vectors: usize,
    /// Whether the field has enough vectors for the approximate preview to be
    /// meaningful (at or above the minimum-dataset threshold). When false, an
    /// approximate request uses exact search per the query's `ann_fallback`.
    pub ann_preview_eligible: bool,
    /// Human-readable preview status: `ready_on_use` (eligible; the graph builds
    /// in memory on first preview query), or `exact_only_below_threshold`.
    pub ann_preview_status: String,
    /// The durable ANN preview generation marker loaded from the last index
    /// snapshot, if any. `None` when the indexes were rebuilt from storage rather
    /// than loaded (the graph is always rebuildable from the exact vectors).
    pub ann_generation: Option<u64>,
}

/// Per-collection search-index report (full-text BM25 and exact vector indexes).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchIndexInfo {
    /// The collection name.
    pub collection: String,
    /// Full-text (BM25) index summaries.
    pub text_fields: Vec<TextIndexInfo>,
    /// Exact vector index summaries.
    pub vector_fields: Vec<VectorIndexInfo>,
}

struct Inner {
    storage: Storage,
    indexes: HashMap<String, CollectionIndexes>,
    clock: LogicalClock,
    index_dir: PathBuf,
    load_report: IndexLoadReport,
    /// The active-transaction registry, keyed by transaction id. The single
    /// source of truth for pinned snapshots: GC preserves every version any
    /// `Active` entry can observe, and the abandoned-transaction reaper releases
    /// entries idle past the timeout.
    txns: BTreeMap<u64, TxnEntry>,
    /// Wall-clock time source for transaction lifecycle timestamps and timeouts.
    wall_clock: WallClock,
    /// Idle timeout after which an unfinished transaction is reaped (`0` = off).
    transaction_timeout_secs: u64,
    /// Cumulative count of transactions reaped for exceeding the idle timeout.
    transaction_timeouts_total: u64,
    /// Minimum versions per live record GC retains (from [`EngineOptions`]).
    gc_min_retained_versions: usize,
    /// Persisted planner statistics (advisory; refreshed by `analyze` and
    /// compaction, with row counts kept current on each mutation).
    planner_stats: PlannerStats,
    /// Path to the persisted planner statistics file.
    stats_path: PathBuf,
    /// When set, data-plane commits are routed through this replicated log
    /// (cluster mode). `None` is the default, single-node direct write path.
    cluster: Option<Arc<dyn ReplicatedLog>>,
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
    fn stats(&self, collection: &str) -> Option<&CollectionStats> {
        self.planner_stats.get(collection)
    }
    fn stats_version(&self) -> Option<u32> {
        Some(self.planner_stats.format_version)
    }
}

/// Iterate a collection's records *as seen within a transaction*: the records
/// visible at the transaction's pinned snapshot (`read_ts`) overlaid with the
/// transaction's staged puts, with staged deletes and put-shadowed snapshot
/// records removed. This is the primitive that gives transactional reads both
/// snapshot isolation and read-your-writes semantics.
fn overlay_scan<'a>(
    inner: &'a Inner,
    txn: &'a Transaction,
    collection: &str,
) -> impl Iterator<Item = &'a Record> + 'a {
    let cid = CollectionId::new(collection.to_string());
    let scan_cid = cid.clone();
    // Records visible at the snapshot, minus any the transaction has staged (a
    // staged put shadows the snapshot version; a staged delete removes it).
    let committed = inner
        .storage
        .scan_as_of(&scan_cid, txn.read_ts())
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
            None => self.inner.storage.get_as_of(
                &CollectionId::new(collection.to_string()),
                id,
                self.txn.read_ts(),
            ),
        }
    }

    fn resolve_link(&self, target: &str, key: &Value) -> Option<&Record> {
        let id = crate::idgen::derive_id(target, key);
        self.get(target, id)
    }

    fn stats(&self, collection: &str) -> Option<&CollectionStats> {
        self.inner.planner_stats.get(collection)
    }
    fn stats_version(&self) -> Option<u32> {
        Some(self.inner.planner_stats.format_version)
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
        let stats_path = dir.join("planner_stats.json");
        let planner_stats = PlannerStats::load(&stats_path);

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
                txns: BTreeMap::new(),
                wall_clock: options.clock,
                transaction_timeout_secs: options.transaction_timeout_secs,
                transaction_timeouts_total: 0,
                gc_min_retained_versions: options.gc_min_retained_versions.max(1),
                planner_stats,
                stats_path,
                cluster: None,
            })),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("engine mutex poisoned")
    }

    /// Route this engine's data-plane commits through a replicated log (enable
    /// cluster mode). After this call, every commit is appended to `log` and
    /// applied to storage at the committed log index; writes on a non-leader
    /// node are rejected with [`Error::NotLeader`].
    ///
    /// This is a no-op on behavior until called; the default engine path is the
    /// unchanged single-node direct write path.
    pub fn attach_replicated_log(&self, log: Arc<dyn ReplicatedLog>) {
        self.lock().cluster = Some(log);
    }

    /// The current MVCC commit watermark — for the replication layer, this is the
    /// highest applied log index, so it knows which committed entries still need
    /// to be replayed after a restart.
    pub fn commit_watermark(&self) -> u64 {
        self.lock().storage.commit_watermark()
    }

    /// Apply a committed replicated batch at `log_index` (used by followers and
    /// by crash-recovery replay). Idempotent: a `log_index` at or below the
    /// current watermark is ignored, so replaying the log is always safe.
    ///
    /// Unlike the leader write path, this recomputes index deltas from current
    /// storage state rather than from a staged write set, because a follower
    /// never ran the originating mutation locally.
    pub fn apply_replicated_batch(&self, batch: Batch, log_index: u64) -> Result<()> {
        let mut inner = self.lock();
        inner.apply_replicated_batch(batch, log_index)
    }

    /// Install a state-machine snapshot into this live engine at the snapshot's
    /// boundary `log_index` (the follower-side of peer snapshot install).
    ///
    /// The snapshot's committed state — collection schemas plus their current
    /// records — replaces what the engine holds and the commit watermark advances
    /// to `log_index`. This **bypasses** the replication log (the snapshot is
    /// already-committed leader state), so it is safe to call on a follower whose
    /// replicated write log is attached, where a normal `apply_mutation` would be
    /// rejected as `not_leader`. Idempotent: a `log_index` at or below the current
    /// watermark is ignored.
    ///
    /// This targets the preview's fail-stop recovery case where the follower is
    /// strictly behind the snapshot. It upserts the snapshot's records over any
    /// existing state rather than reconciling divergent history, which is correct
    /// when the follower's state is a prefix of the snapshot (a follower that only
    /// fell behind), and is documented as a preview limitation otherwise.
    pub fn install_snapshot(
        &self,
        schemas: Vec<CollectionSchema>,
        records: Vec<(String, Document)>,
        log_index: u64,
    ) -> Result<()> {
        let mut inner = self.lock();
        if log_index <= inner.storage.commit_watermark() {
            return Ok(());
        }
        // Install schemas first so the record upserts below have somewhere to go
        // and indexes/planner stats are initialized.
        for schema in schemas {
            schema.validate_definition()?;
            let name = schema.name.clone();
            inner.storage.put_schema(schema.clone())?;
            let mut idx = CollectionIndexes::from_schema(&schema);
            idx.rebuild(inner.storage.scan(&CollectionId::new(name.clone())))?;
            inner.indexes.insert(name.clone(), idx);
            let stats = CollectionStats::compute(
                &schema,
                inner.storage.scan(&CollectionId::new(name.clone())),
            );
            inner.planner_stats.collections.insert(name, stats);
        }
        // Build one committed batch of record upserts and apply it at the
        // snapshot boundary, reusing the idempotent committed-apply path that
        // maintains indexes, planner stats, and the commit watermark.
        let mut ops = Vec::with_capacity(records.len());
        for (collection, fields) in records {
            let schema = inner.schema_for(&collection)?;
            let cid = CollectionId::new(collection.clone());
            let id = record_id_for(&schema, &fields, inner.next_id())?;
            let existing = inner.storage.get(&cid, id).cloned();
            let version = existing.as_ref().map(|r| r.version + 1).unwrap_or(1);
            ops.push(LogOp::Put {
                commit_ts: 0,
                record: Record {
                    id,
                    collection: cid,
                    fields,
                    version,
                    created_txn: TxnId::AUTO,
                },
            });
        }
        inner.apply_replicated_batch(
            Batch {
                txn_id: TxnId::AUTO,
                ops,
            },
            log_index,
        )?;
        // Refresh planner statistics so the restored engine plans like a loaded one.
        inner.analyze_all()
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
        inner.indexes.insert(name.clone(), idx);
        // Initialize/refresh planner statistics for the (possibly repopulated)
        // collection so the planner has a row count immediately.
        let stats = CollectionStats::compute(
            &schema,
            inner.storage.scan(&CollectionId::new(name.clone())),
        );
        inner.planner_stats.collections.insert(name, stats);
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

    /// Report the search indexes (full-text BM25 and exact vector) across all
    /// collections, with their statistics, for `auradb index check` and
    /// `auradb doctor`. A BM25 index with documents but a zero average length
    /// signals missing length statistics that a rebuild would repair.
    pub fn search_index_report(&self) -> Vec<SearchIndexInfo> {
        let inner = self.lock();
        let mut out = Vec::new();
        for schema in inner.storage.list_schemas() {
            let Some(indexes) = inner.indexes.get(&schema.name) else {
                continue;
            };
            let mut text_fields = Vec::new();
            for field in indexes.text_field_names() {
                if let Some(s) = indexes.text_index_stats(field) {
                    let warning = if s.documents > 0 && s.avg_doc_len <= 0.0 {
                        Some("missing length statistics; run `auradb index rebuild`".to_string())
                    } else {
                        None
                    };
                    text_fields.push(TextIndexInfo {
                        field: field.to_string(),
                        documents: s.documents,
                        distinct_terms: s.distinct_terms,
                        avg_doc_len: s.avg_doc_len,
                        warning,
                    });
                }
            }
            text_fields.sort_by(|a, b| a.field.cmp(&b.field));
            let loaded_meta = indexes.loaded_ann_metadata();
            let mut vector_fields: Vec<VectorIndexInfo> = indexes
                .vector_field_stats()
                .map(|(field, dim, count)| {
                    let eligible = count >= query::ANN_PREVIEW_MIN_VECTORS;
                    VectorIndexInfo {
                        field: field.to_string(),
                        dim,
                        vectors: count,
                        ann_preview_eligible: eligible,
                        ann_preview_status: if eligible {
                            "ready_on_use".to_string()
                        } else {
                            "exact_only_below_threshold".to_string()
                        },
                        ann_generation: loaded_meta
                            .iter()
                            .find(|m| m.field == field)
                            .map(|m| m.generation),
                    }
                })
                .collect();
            vector_fields.sort_by(|a, b| a.field.cmp(&b.field));
            if !text_fields.is_empty() || !vector_fields.is_empty() {
                out.push(SearchIndexInfo {
                    collection: schema.name.clone(),
                    text_fields,
                    vector_fields,
                });
            }
        }
        out
    }

    // ----- reads -----

    /// Plan a find and return ordered ids/scores plus the EXPLAIN plan.
    pub fn plan_find(&self, q: &FindQuery) -> Result<query::PlannedFind> {
        let inner = self.lock();
        query::execute_find(&*inner, q)
    }

    /// Plan a find under an explicit cooperative [`query::Deadline`], overriding
    /// the query's own `timeout_ms`. A query that runs past the deadline returns
    /// a structured `query_timeout` error; the engine and session stay usable.
    pub fn plan_find_within(
        &self,
        q: &FindQuery,
        deadline: &query::Deadline,
    ) -> Result<query::PlannedFind> {
        let inner = self.lock();
        query::execute_find_within(&*inner, q, deadline)
    }

    /// Materialize specific rows of a find (used for cursor paging).
    pub fn materialize(&self, q: &FindQuery, page: &[Scored]) -> Result<Vec<Row>> {
        let inner = self.lock();
        query::materialize(&*inner, q, page)
    }

    /// Materialize a page of a find with a starting rank offset (cursor paging),
    /// so each row's reported `rank` is stable across pages.
    pub fn materialize_page(
        &self,
        q: &FindQuery,
        page: &[Scored],
        start_rank: usize,
    ) -> Result<Vec<Row>> {
        let inner = self.lock();
        query::materialize_page(&*inner, q, page, start_rank)
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

    /// Compute aggregations and/or terms facets over a collection. The query's
    /// `timeout_ms` bounds execution cooperatively (the server clamps it against
    /// the configured maximum before calling).
    pub fn aggregate(&self, q: &query::AggregateQuery) -> Result<query::AggregateResult> {
        let inner = self.lock();
        let deadline = query::Deadline::after_ms(q.timeout_ms.unwrap_or(0));
        query::execute_aggregate(&*inner, q, &deadline)
    }

    /// Compute aggregations/facets under an explicit cooperative deadline,
    /// overriding the query's own `timeout_ms`. A query that runs past the
    /// deadline returns a structured `query_timeout` error.
    pub fn aggregate_within(
        &self,
        q: &query::AggregateQuery,
        deadline: &query::Deadline,
    ) -> Result<query::AggregateResult> {
        let inner = self.lock();
        query::execute_aggregate(&*inner, q, deadline)
    }

    // ----- auto-commit mutations -----

    /// Apply a mutation in auto-commit mode.
    pub fn apply_mutation(&self, mutation: Mutation) -> Result<MutationResult> {
        let mut inner = self.lock();
        let txn_id = TxnId::AUTO;
        let collection = mutation.collection().to_string();
        let result = match mutation {
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
        }?;
        inner.refresh_stats_row_count(&collection);
        Ok(result)
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

    /// Begin a transaction, pinning its snapshot read timestamp at the current
    /// commit watermark. All reads within the transaction see committed state as
    /// of this point in MVCC time (overlaid with the transaction's own writes).
    pub fn begin(&self) -> Transaction {
        self.begin_with_connection(None)
    }

    /// Begin a transaction owned by a connection. The connection id is recorded
    /// in the active-transaction registry for observability and so the server
    /// can attribute and clean up a connection's transactions on disconnect.
    pub fn begin_with_connection(&self, connection_id: Option<u64>) -> Transaction {
        let mut inner = self.lock();
        let read_ts = inner.storage.commit_watermark();
        let id = TxnId(inner.clock.tick());
        let now = inner.wall_clock.now_secs();
        inner.txns.insert(
            id.get(),
            TxnEntry {
                read_ts,
                started_at: now,
                last_activity: now,
                connection_id,
                state: TxnState::Active,
            },
        );
        Transaction::begin_at(id, read_ts)
    }

    /// Reap transactions idle longer than the configured timeout: mark each
    /// `TimedOut`, release its pinned snapshot (so GC can progress), and count
    /// it. Returns the number reaped. A no-op when the timeout is disabled.
    /// Driven by the server's abandoned-transaction reaper task, and callable
    /// directly (with a manual clock) in tests.
    pub fn reap_transactions(&self) -> usize {
        self.lock().reap_transactions()
    }

    /// A point-in-time snapshot of every registered transaction, for status and
    /// observability output.
    pub fn active_transactions(&self) -> Vec<ActiveTransaction> {
        let inner = self.lock();
        let now = inner.wall_clock.now_secs();
        inner
            .txns
            .iter()
            .map(|(id, e)| ActiveTransaction {
                id: *id,
                read_ts: e.read_ts,
                age_secs: now.saturating_sub(e.started_at),
                idle_secs: now.saturating_sub(e.last_activity),
                connection_id: e.connection_id,
                state: e.state,
            })
            .collect()
    }

    /// Stage a mutation within a transaction (no durable write yet).
    pub fn stage(&self, txn: &mut Transaction, mutation: Mutation) -> Result<MutationResult> {
        let mut inner = self.lock();
        inner.touch_txn(txn.id().get())?;
        inner.stage_mutation(txn, mutation)
    }

    /// Read a record within a transaction, honoring staged writes
    /// (read-your-writes).
    pub fn txn_get(&self, txn: &Transaction, collection: &str, id: RecordId) -> Option<Record> {
        let key = Key::new(CollectionId::new(collection.to_string()), id);
        match txn.staged(&key) {
            Some(StagedOp::Put(r)) => Some(r.clone()),
            Some(StagedOp::Delete) => None,
            None => self
                .lock()
                .storage
                .get_as_of(
                    &CollectionId::new(collection.to_string()),
                    id,
                    txn.read_ts(),
                )
                .cloned(),
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
        let mut inner = self.lock();
        inner.touch_txn(txn.id().get())?;
        let view = inner.txn_view(txn, &q.collection)?;
        query::execute_find(&view, q)
    }

    /// Materialize specific rows of a transactional find (used for cursor
    /// paging within a transaction).
    pub fn txn_materialize(
        &self,
        txn: &Transaction,
        q: &FindQuery,
        page: &[Scored],
    ) -> Result<Vec<Row>> {
        let mut inner = self.lock();
        inner.touch_txn(txn.id().get())?;
        let view = inner.txn_view(txn, &q.collection)?;
        query::materialize(&view, q, page)
    }

    /// Materialize a page of a transactional find with a starting rank offset.
    pub fn txn_materialize_page(
        &self,
        txn: &Transaction,
        q: &FindQuery,
        page: &[Scored],
        start_rank: usize,
    ) -> Result<Vec<Row>> {
        let mut inner = self.lock();
        inner.touch_txn(txn.id().get())?;
        let view = inner.txn_view(txn, &q.collection)?;
        query::materialize_page(&view, q, page, start_rank)
    }

    /// Run a find to completion within a transaction, returning all matching
    /// rows from the transaction view.
    pub fn txn_find(&self, txn: &Transaction, q: &FindQuery) -> Result<Vec<Row>> {
        let mut inner = self.lock();
        inner.touch_txn(txn.id().get())?;
        let view = inner.txn_view(txn, &q.collection)?;
        let planned = query::execute_find(&view, q)?;
        query::materialize(&view, q, &planned.ordered)
    }

    /// Count matching records within a transaction.
    pub fn txn_count(&self, txn: &Transaction, q: &CountQuery) -> Result<usize> {
        let mut inner = self.lock();
        inner.touch_txn(txn.id().get())?;
        let view = inner.txn_view(txn, &q.collection)?;
        query::execute_count(&view, q)
    }

    /// Test whether any record matches within a transaction.
    pub fn txn_exists(&self, txn: &Transaction, q: &ExistsQuery) -> Result<bool> {
        let mut inner = self.lock();
        inner.touch_txn(txn.id().get())?;
        let view = inner.txn_view(txn, &q.collection)?;
        query::execute_exists(&view, q)
    }

    /// Page a ranked search (vector / text_search / hybrid) by stable keyset
    /// cursor token. `cursor` is `None` for the first page or a token from a
    /// previous call. Returns the page's rows (with stable cross-page ranks) and
    /// the next-page token (`None` on the last page). The query's `timeout_ms`
    /// bounds each page's evaluation.
    pub fn search_page(
        &self,
        q: &FindQuery,
        page_size: usize,
        cursor: Option<&str>,
    ) -> Result<(Vec<Row>, Option<String>)> {
        let inner = self.lock();
        let deadline = query::Deadline::after_ms(q.timeout_ms.unwrap_or(0));
        let page = query::paginate_ranked(&*inner, q, page_size, cursor, &deadline)?;
        let rows = query::materialize_page(&*inner, q, &page.rows, page.start_rank)?;
        Ok((rows, page.next_cursor))
    }

    /// Page a ranked search within a transaction view. Paging inside a
    /// transaction fixes the snapshot (and thus the corpus statistics), so
    /// BM25/hybrid cursors are duplicate-stable across concurrent writes.
    pub fn txn_search_page(
        &self,
        txn: &Transaction,
        q: &FindQuery,
        page_size: usize,
        cursor: Option<&str>,
    ) -> Result<(Vec<Row>, Option<String>)> {
        let mut inner = self.lock();
        inner.touch_txn(txn.id().get())?;
        let view = inner.txn_view(txn, &q.collection)?;
        let deadline = query::Deadline::after_ms(q.timeout_ms.unwrap_or(0));
        let page = query::paginate_ranked(&view, q, page_size, cursor, &deadline)?;
        let rows = query::materialize_page(&view, q, &page.rows, page.start_rank)?;
        Ok((rows, page.next_cursor))
    }

    /// Compute aggregations/facets within a transaction view.
    pub fn txn_aggregate(
        &self,
        txn: &Transaction,
        q: &query::AggregateQuery,
    ) -> Result<query::AggregateResult> {
        let mut inner = self.lock();
        inner.touch_txn(txn.id().get())?;
        let view = inner.txn_view(txn, &q.collection)?;
        let deadline = query::Deadline::after_ms(q.timeout_ms.unwrap_or(0));
        query::execute_aggregate(&view, q, &deadline)
    }

    /// Produce an EXPLAIN plan for a find within a transaction.
    pub fn txn_explain(&self, txn: &Transaction, q: &FindQuery) -> Result<ExplainPlan> {
        let mut inner = self.lock();
        inner.touch_txn(txn.id().get())?;
        let view = inner.txn_view(txn, &q.collection)?;
        query::explain(&view, q)
    }

    /// Commit a transaction atomically, with snapshot-isolation write-conflict
    /// detection. On conflict the transaction is aborted and its snapshot
    /// released.
    pub fn commit(&self, txn: Transaction) -> Result<()> {
        let id = txn.id().get();
        let mut inner = self.lock();
        // A transaction reaped for timeout cannot commit: its snapshot was
        // already released, so its first-committer-wins conflict check is no
        // longer sound. Reject with a structured timeout error and clean up.
        if matches!(
            inner.txns.get(&id).map(|e| e.state),
            Some(TxnState::TimedOut)
        ) {
            inner.txns.remove(&id);
            return Err(Error::TransactionTimeout(format!(
                "transaction {id} timed out before commit and was aborted"
            )));
        }
        let result = inner.commit_transaction(txn);
        inner.txns.remove(&id);
        result
    }

    /// Roll back a transaction, discarding all staged writes and releasing its
    /// snapshot. Rolling back a timed-out transaction is accepted (it is the
    /// connection cleaning up after the reaper) and simply unregisters it.
    pub fn rollback(&self, mut txn: Transaction) {
        let id = txn.id().get();
        txn.finish();
        drop(txn);
        self.lock().txns.remove(&id);
    }

    /// Reclaim MVCC versions no active transaction can observe.
    ///
    /// The reclamation horizon is the oldest read timestamp pinned by an active
    /// transaction, or the commit watermark when none are active. Old versions
    /// older than the horizon are removed (always keeping the latest, and at
    /// least the configured minimum), and fully-deleted records are dropped. The
    /// latest version of every live record is preserved, so indexes — which
    /// reflect only latest live state — need no rebuild.
    pub fn gc(&self) -> Result<auradb_storage::GcReport> {
        let mut inner = self.lock();
        let cutoff = inner.gc_cutoff();
        let min_retained = inner.gc_min_retained_versions;
        inner.storage.gc(cutoff, min_retained)
    }

    /// Report what [`gc`](Self::gc) would reclaim at the current horizon without
    /// modifying any data. Used by `auradb gc --dry-run`.
    pub fn gc_dry_run(&self) -> auradb_storage::GcReport {
        let inner = self.lock();
        let cutoff = inner.gc_cutoff();
        inner
            .storage
            .gc_preview(cutoff, inner.gc_min_retained_versions)
    }

    /// Recompute and persist planner statistics (`analyze`). Statistics are
    /// advisory: they change which plan the planner chooses, never query results.
    pub fn analyze(&self) -> Result<()> {
        self.lock().analyze_all()
    }

    /// A snapshot of the current planner statistics (for `stats show`).
    pub fn planner_stats(&self) -> PlannerStats {
        self.lock().planner_stats.clone()
    }

    /// Produce an `EXPLAIN ANALYZE` plan: run the find and attach measured
    /// execution metrics.
    pub fn explain_analyze(&self, q: &FindQuery) -> Result<ExplainPlan> {
        let inner = self.lock();
        query::explain_analyze(&*inner, q, None)
    }

    /// Produce an `EXPLAIN ANALYZE` plan within a transaction; reports the
    /// snapshot read timestamp.
    pub fn txn_explain_analyze(&self, txn: &Transaction, q: &FindQuery) -> Result<ExplainPlan> {
        let mut inner = self.lock();
        inner.touch_txn(txn.id().get())?;
        let view = inner.txn_view(txn, &q.collection)?;
        query::explain_analyze(&view, q, Some(txn.read_ts()))
    }

    // ----- maintenance -----

    /// Compact storage, preserving all live data, then refresh persisted index
    /// snapshots so a subsequent open loads them directly.
    pub fn compact(&self) -> Result<auradb_storage::CompactionReport> {
        let mut inner = self.lock();
        let report = inner.storage.compact()?;
        inner.persist_indexes()?;
        // Compaction is a natural point to refresh and persist planner stats.
        inner.analyze_all()?;
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
        inner.persist_indexes()?;
        inner.persist_stats()
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
        let now = inner.wall_clock.now_secs();
        let active_transactions = inner
            .txns
            .values()
            .filter(|e| e.state == TxnState::Active)
            .count();
        let timed_out_transactions = inner
            .txns
            .values()
            .filter(|e| e.state == TxnState::TimedOut)
            .count();
        let oldest_transaction_age_secs = inner
            .txns
            .values()
            .filter(|e| e.state == TxnState::Active)
            .map(|e| now.saturating_sub(e.started_at))
            .max();
        EngineStats {
            collections: inner.storage.collection_count(),
            records: inner.storage.total_records(),
            versions: inner.storage.total_versions(),
            active_transactions,
            timed_out_transactions,
            oldest_active_read_ts: inner.oldest_active_read_ts(),
            oldest_transaction_age_secs,
            transaction_timeouts_total: inner.transaction_timeouts_total,
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
    /// The single choke point through which every data-plane commit flows.
    ///
    /// In single-node mode this delegates straight to storage. In cluster mode
    /// it first appends the batch to the replicated log, waits for consensus to
    /// commit it, and then applies it to storage at the committed log index
    /// (used as the commit timestamp). A non-leader returns [`Error::NotLeader`].
    fn commit_batch(&mut self, batch: Batch) -> Result<u64> {
        match self.cluster.clone() {
            Some(log) => {
                let index = log.replicate(&batch)?;
                self.storage.apply_committed_batch(batch, index)?;
                Ok(index)
            }
            None => self.storage.commit_batch(batch),
        }
    }

    /// Apply a committed replicated batch to storage and indexes at `log_index`.
    /// Idempotent on the commit watermark; see [`Engine::apply_replicated_batch`].
    fn apply_replicated_batch(&mut self, batch: Batch, log_index: u64) -> Result<()> {
        if log_index <= self.storage.commit_watermark() {
            return Ok(());
        }
        // Capture the prior state of every touched record so index deltas can be
        // computed before the batch mutates storage.
        let mut deltas: Vec<(Option<Record>, Option<Record>)> = Vec::new();
        for op in &batch.ops {
            match op {
                LogOp::Put { record, .. } => {
                    let existing = self.storage.get(&record.collection, record.id).cloned();
                    deltas.push((existing, Some(record.clone())));
                }
                LogOp::Delete { collection, id, .. } => {
                    let existing = self.storage.get(collection, *id).cloned();
                    deltas.push((existing, None));
                }
            }
        }
        self.storage.apply_committed_batch(batch, log_index)?;

        let mut affected: HashSet<String> = HashSet::new();
        for (old, new) in deltas {
            let collection = old
                .as_ref()
                .or(new.as_ref())
                .map(|r| r.collection.0.clone())
                .expect("a replicated op always names a record");
            if let Some(idx) = self.indexes.get_mut(&collection) {
                if let Some(old) = &old {
                    idx.remove(old);
                }
                if let Some(new) = &new {
                    idx.insert(new);
                }
            }
            affected.insert(collection);
        }
        for collection in affected {
            self.refresh_stats_row_count(&collection);
        }
        Ok(())
    }

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

    /// Record activity on a transaction and reject operations on one that has
    /// timed out. Updates `last_activity` for an active transaction. An id the
    /// registry does not know (an embedded `Transaction` built without `begin`,
    /// or one already finished) is treated as a no-op so direct embedded use
    /// keeps working.
    fn touch_txn(&mut self, id: u64) -> Result<()> {
        let now = self.wall_clock.now_secs();
        match self.txns.get_mut(&id) {
            Some(entry) => match entry.state {
                TxnState::Active => {
                    entry.last_activity = now;
                    Ok(())
                }
                TxnState::TimedOut => Err(Error::TransactionTimeout(format!(
                    "transaction {id} exceeded its idle timeout and was aborted"
                ))),
            },
            None => Ok(()),
        }
    }

    /// Reap transactions idle past the configured timeout. Each is marked
    /// `TimedOut` (so further operations are rejected) and its snapshot is no
    /// longer counted toward the GC horizon, letting GC progress.
    fn reap_transactions(&mut self) -> usize {
        if self.transaction_timeout_secs == 0 {
            return 0;
        }
        let now = self.wall_clock.now_secs();
        let timeout = self.transaction_timeout_secs;
        let mut reaped = 0;
        for entry in self.txns.values_mut() {
            if entry.state == TxnState::Active && now.saturating_sub(entry.last_activity) >= timeout
            {
                entry.state = TxnState::TimedOut;
                reaped += 1;
            }
        }
        self.transaction_timeouts_total += reaped as u64;
        reaped
    }

    /// The smallest read timestamp pinned by an `Active` transaction, if any.
    fn oldest_active_read_ts(&self) -> Option<u64> {
        self.txns
            .values()
            .filter(|e| e.state == TxnState::Active)
            .map(|e| e.read_ts)
            .min()
    }

    /// The GC reclamation horizon: the oldest snapshot pinned by an active
    /// transaction, or the commit watermark when none are active. Timed-out
    /// transactions are excluded, so reaping an abandoned transaction lets GC
    /// reclaim the versions it had pinned.
    fn gc_cutoff(&self) -> u64 {
        self.oldest_active_read_ts()
            .unwrap_or_else(|| self.storage.commit_watermark())
    }

    /// Keep a collection's planner row count current after a mutation. Cheap:
    /// reads the live count for one collection. Cardinality is refreshed only by
    /// [`Inner::analyze_all`].
    fn refresh_stats_row_count(&mut self, collection: &str) {
        let count = self
            .storage
            .count(&CollectionId::new(collection.to_string()));
        self.planner_stats
            .collections
            .entry(collection.to_string())
            .or_default()
            .row_count = count;
    }

    /// Recompute full planner statistics for every collection from the latest
    /// committed state and persist them.
    fn analyze_all(&mut self) -> Result<()> {
        let schemas: Vec<CollectionSchema> =
            self.storage.list_schemas().into_iter().cloned().collect();
        let mut stats = PlannerStats::default();
        for schema in &schemas {
            let cid = CollectionId::new(schema.name.clone());
            let computed = CollectionStats::compute(schema, self.storage.scan(&cid));
            stats.collections.insert(schema.name.clone(), computed);
        }
        self.planner_stats = stats;
        self.persist_stats()
    }

    /// Persist planner statistics to disk (advisory; best-effort durability).
    fn persist_stats(&self) -> Result<()> {
        self.planner_stats.save(&self.stats_path)
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
            .map(|(_, r)| LogOp::Put {
                commit_ts: 0,
                record: r.clone(),
            })
            .collect();
        self.commit_batch(Batch { txn_id, ops })?;

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
            .map(|(_, r)| LogOp::Put {
                commit_ts: 0,
                record: r.clone(),
            })
            .collect();
        self.commit_batch(Batch { txn_id, ops })?;

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
                commit_ts: 0,
                collection: cid.clone(),
                id: r.id,
            })
            .collect();
        self.commit_batch(Batch { txn_id, ops })?;

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

    /// A single record as the transaction sees it: its own staged write if any,
    /// otherwise the version visible at its pinned snapshot. Cloned so the caller
    /// may then mutate the transaction.
    fn txn_get_visible(
        &self,
        txn: &Transaction,
        cid: &CollectionId,
        id: RecordId,
    ) -> Option<Record> {
        match txn.staged(&Key::new(cid.clone(), id)) {
            Some(StagedOp::Put(r)) => Some(r.clone()),
            Some(StagedOp::Delete) => None,
            None => self.storage.get_as_of(cid, id, txn.read_ts()).cloned(),
        }
    }

    /// All records in a collection as the transaction sees them (snapshot +
    /// staged overlay), cloned.
    fn txn_scan_visible(&self, txn: &Transaction, collection: &str) -> Vec<Record> {
        overlay_scan(self, txn, collection).cloned().collect()
    }

    fn stage_mutation(&self, txn: &mut Transaction, mutation: Mutation) -> Result<MutationResult> {
        let collection = mutation.collection().to_string();
        let schema = self.schema_for(&collection)?;
        let cid = CollectionId::new(collection.clone());
        let mut result = MutationResult::empty();

        match mutation {
            Mutation::Insert { fields, .. } | Mutation::Upsert { fields, .. } => {
                schema.validate_record(&fields)?;
                let id = record_id_for(&schema, &fields, self.clock.tick())?;
                let existing = self.txn_get_visible(txn, &cid, id);
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
                    let existing = self.txn_get_visible(txn, &cid, id);
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
                    .txn_scan_visible(txn, &collection)
                    .into_iter()
                    .filter(|r| {
                        filter
                            .as_ref()
                            .map(|f| query::eval::matches(r, f))
                            .unwrap_or(true)
                    })
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
                    .txn_scan_visible(txn, &collection)
                    .into_iter()
                    .filter(|r| {
                        filter
                            .as_ref()
                            .map(|f| query::eval::matches(r, f))
                            .unwrap_or(true)
                    })
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
        let read_ts = txn.read_ts();
        let (_observed, staged) = txn.into_parts();

        // 1. Snapshot-isolation write-conflict detection (first-committer-wins).
        // The transaction aborts if any record it wrote has a committed version
        // (live or tombstone) newer than its snapshot — i.e. another transaction
        // committed a conflicting write to the same key after this one began.
        // This covers write-write, update-delete, and delete-update conflicts.
        for key in staged.keys() {
            if let Some(latest) = self.storage.latest_commit_ts(&key.collection, key.id) {
                if latest > read_ts {
                    return Err(Error::Conflict(format!(
                        "record {} in {} was modified by a concurrent transaction",
                        key.id, key.collection
                    )));
                }
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
                        commit_ts: 0,
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
                        commit_ts: 0,
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

        // 3. Durable atomic commit (routed through the replicated log in
        //    cluster mode; a non-leader is rejected here before any apply).
        self.commit_batch(Batch {
            txn_id,
            ops: batch_ops,
        })?;

        // 4. Update indexes.
        let mut affected: HashSet<String> = HashSet::new();
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
            affected.insert(collection);
        }

        // 5. Keep planner row counts current for the collections written.
        for collection in affected {
            self.refresh_stats_row_count(&collection);
        }
        Ok(())
    }
}
