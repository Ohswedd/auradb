//! # auradb-txn
//!
//! Single-node transactions implemented as in-memory **staged write sets** with
//! **optimistic conflict detection**.
//!
//! A transaction pins a **read timestamp** (`read_ts`) at begin: the MVCC commit
//! watermark at that moment. Every read inside the transaction sees the database
//! as of `read_ts` (the engine resolves it against storage version chains),
//! overlaid with the transaction's own staged writes (read-your-writes).
//! Mutations are staged (not visible to other transactions). At commit time the
//! engine, under a single write lock, rejects the transaction if any record it
//! wrote has a committed version newer than `read_ts` (first-committer-wins
//! write-conflict detection); otherwise the staged operations are turned into an
//! atomic storage [`Batch`] and applied at a fresh commit timestamp.
//!
//! Isolation level: **single-node snapshot isolation** with optimistic write
//! conflict detection. This is **not** serializable isolation and is documented
//! as such (`docs/TRANSACTIONS.md`). Distributed transactions are not
//! implemented.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;

use auradb_core::{CollectionId, Record, RecordId, TxnId};
use auradb_storage::{Batch, LogOp};

/// A record identity within a transaction.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// The collection.
    pub collection: CollectionId,
    /// The record id.
    pub id: RecordId,
}

impl Key {
    /// Construct a key.
    pub fn new(collection: CollectionId, id: RecordId) -> Self {
        Key { collection, id }
    }
}

/// A staged mutation within a transaction.
#[derive(Debug, Clone, PartialEq)]
pub enum StagedOp {
    /// Insert or replace a record.
    Put(Record),
    /// Delete a record.
    Delete,
}

/// An in-progress transaction.
#[derive(Debug)]
pub struct Transaction {
    id: TxnId,
    /// The MVCC read timestamp pinned at begin: all reads see committed state as
    /// of this point in time.
    read_ts: u64,
    /// Versions observed when each key was first read or written.
    /// `None` means the key was observed absent.
    observed: BTreeMap<Key, Option<u64>>,
    /// Staged, not-yet-committed mutations.
    staged: BTreeMap<Key, StagedOp>,
    finished: bool,
}

impl Transaction {
    /// Begin a new transaction with the given id, pinning `read_ts` as its
    /// snapshot timestamp (the commit watermark observed at begin).
    pub fn begin_at(id: TxnId, read_ts: u64) -> Self {
        Transaction {
            id,
            read_ts,
            observed: BTreeMap::new(),
            staged: BTreeMap::new(),
            finished: false,
        }
    }

    /// Begin a new transaction with the given id and a zero read timestamp.
    /// Prefer [`Transaction::begin_at`]; this exists for tests and tooling that
    /// do not pin a snapshot.
    pub fn begin(id: TxnId) -> Self {
        Transaction::begin_at(id, 0)
    }

    /// The transaction id.
    pub fn id(&self) -> TxnId {
        self.id
    }

    /// The pinned MVCC read timestamp (snapshot) for this transaction.
    pub fn read_ts(&self) -> u64 {
        self.read_ts
    }

    /// Whether the transaction has been committed or rolled back.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Whether the transaction has staged any writes.
    pub fn is_empty(&self) -> bool {
        self.staged.is_empty()
    }

    /// Record the version observed for a key (first observation wins). Pass the
    /// committed version, or `None` if the record was absent.
    pub fn observe(&mut self, key: Key, version: Option<u64>) {
        self.observed.entry(key).or_insert(version);
    }

    /// Stage a put. Records the prior observed version for conflict detection.
    pub fn stage_put(&mut self, record: Record, prior_version: Option<u64>) {
        let key = Key::new(record.collection.clone(), record.id);
        self.observe(key.clone(), prior_version);
        self.staged.insert(key, StagedOp::Put(record));
    }

    /// Stage a delete. Records the prior observed version for conflict detection.
    pub fn stage_delete(
        &mut self,
        collection: CollectionId,
        id: RecordId,
        prior_version: Option<u64>,
    ) {
        let key = Key::new(collection, id);
        self.observe(key.clone(), prior_version);
        self.staged.insert(key, StagedOp::Delete);
    }

    /// Look up a staged mutation for read-your-writes semantics.
    pub fn staged(&self, key: &Key) -> Option<&StagedOp> {
        self.staged.get(key)
    }

    /// The set of observed `(key, version)` pairs to validate at commit.
    pub fn observed(&self) -> impl Iterator<Item = (&Key, &Option<u64>)> {
        self.observed.iter()
    }

    /// The staged mutations in deterministic key order.
    pub fn staged_ops(&self) -> impl Iterator<Item = (&Key, &StagedOp)> {
        self.staged.iter()
    }

    /// Consume the transaction, producing the ordered storage operations.
    /// Versions are not assigned here; the engine assigns them under the commit
    /// lock so they are monotonic with respect to committed state.
    pub fn into_batch(self, version_for: impl Fn(&Key) -> u64) -> Batch {
        let txn_id = self.id;
        let ops = self
            .staged
            .into_iter()
            .map(|(key, op)| match op {
                StagedOp::Put(mut record) => {
                    record.version = version_for(&key);
                    record.created_txn = txn_id;
                    // Storage stamps the MVCC commit timestamp on the whole batch.
                    LogOp::Put {
                        commit_ts: 0,
                        record,
                    }
                }
                StagedOp::Delete => LogOp::Delete {
                    commit_ts: 0,
                    collection: key.collection,
                    id: key.id,
                },
            })
            .collect();
        Batch { txn_id, ops }
    }

    /// Mark the transaction finished (committed or rolled back).
    pub fn finish(&mut self) {
        self.finished = true;
    }

    /// Consume the transaction, returning its observed versions and staged
    /// mutations for the engine to validate and apply under the commit lock.
    pub fn into_parts(self) -> (BTreeMap<Key, Option<u64>>, BTreeMap<Key, StagedOp>) {
        (self.observed, self.staged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auradb_core::{Document, Value};

    fn rec(id: u128, v: i64) -> Record {
        let mut f = Document::new();
        f.insert("v".into(), Value::Int(v));
        Record::new(RecordId::from_u128(id), CollectionId::new("C"), f)
    }

    #[test]
    fn read_your_writes() {
        let mut txn = Transaction::begin(TxnId(1));
        txn.stage_put(rec(1, 10), None);
        let key = Key::new(CollectionId::new("C"), RecordId::from_u128(1));
        match txn.staged(&key) {
            Some(StagedOp::Put(r)) => assert_eq!(r.get("v"), Some(&Value::Int(10))),
            _ => panic!("expected staged put"),
        }
    }

    #[test]
    fn observe_first_wins() {
        let mut txn = Transaction::begin(TxnId(1));
        let key = Key::new(CollectionId::new("C"), RecordId::from_u128(1));
        txn.observe(key.clone(), Some(3));
        txn.observe(key.clone(), Some(99));
        assert_eq!(txn.observed.get(&key), Some(&Some(3)));
    }

    #[test]
    fn into_batch_assigns_versions_and_order() {
        let mut txn = Transaction::begin(TxnId(7));
        txn.stage_put(rec(2, 20), None);
        txn.stage_put(rec(1, 10), None);
        txn.stage_delete(CollectionId::new("C"), RecordId::from_u128(3), Some(1));
        let batch = txn.into_batch(|_| 5);
        assert_eq!(batch.txn_id, TxnId(7));
        // Ordered by key (record id 1, then 2, then delete of 3).
        assert_eq!(batch.ops.len(), 3);
        match &batch.ops[0] {
            LogOp::Put { record, .. } => {
                assert_eq!(record.id, RecordId::from_u128(1));
                assert_eq!(record.version, 5);
                assert_eq!(record.created_txn, TxnId(7));
            }
            _ => panic!(),
        }
    }
}
