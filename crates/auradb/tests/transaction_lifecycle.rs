//! Transaction lifecycle hardening (v0.3.1).
//!
//! These tests exercise the active-transaction registry, the idle timeout, and
//! the abandoned-transaction reaper. They drive a manual [`WallClock`] so a
//! timeout fires instantly and deterministically — no sleeps, no flakiness.
//!
//! AuraDB remains single-node snapshot isolation; these tests assert the
//! operational guardrails around long-lived and abandoned transactions, not any
//! change to the isolation contract.

use auradb::core::{CollectionSchema, Document, Error, FieldDef, FieldType, Value};
use auradb::query::{FindQuery, Mutation};
use auradb::storage::StorageOptions;
use auradb::{Engine, EngineOptions, Transaction, TxnState, WallClock};

fn schema() -> CollectionSchema {
    CollectionSchema::new("Doc")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("views", FieldType::Int))
}

fn doc(id: &str, views: i64) -> Document {
    let mut m = Document::new();
    m.insert("id".into(), Value::Text(id.into()));
    m.insert("views".into(), Value::Int(views));
    m
}

/// Open an engine with a manual clock and the given idle timeout. Returns the
/// clock so a test can advance time and trigger the reaper deterministically.
fn open_with_clock(timeout_secs: u64) -> (tempfile::TempDir, Engine, WallClock) {
    let dir = tempfile::tempdir().unwrap();
    let clock = WallClock::manual();
    let engine = Engine::open_with(
        dir.path(),
        EngineOptions {
            storage: StorageOptions::default(),
            gc_min_retained_versions: 1,
            transaction_timeout_secs: timeout_secs,
            clock: clock.clone(),
        },
    )
    .unwrap();
    engine.create_schema(schema()).unwrap();
    (dir, engine, clock)
}

fn stage_insert(engine: &Engine, txn: &mut Transaction, fields: Document) {
    engine
        .stage(
            txn,
            Mutation::Insert {
                collection: "Doc".into(),
                fields,
            },
        )
        .unwrap();
}

#[test]
fn transaction_unregisters_on_commit() {
    let (_dir, engine, _clock) = open_with_clock(60);
    let mut txn = engine.begin();
    stage_insert(&engine, &mut txn, doc("d1", 1));
    assert_eq!(engine.stats().active_transactions, 1);
    engine.commit(txn).unwrap();
    assert_eq!(engine.stats().active_transactions, 0);
    assert!(engine.active_transactions().is_empty());
}

#[test]
fn transaction_unregisters_on_rollback() {
    let (_dir, engine, _clock) = open_with_clock(60);
    let mut txn = engine.begin();
    stage_insert(&engine, &mut txn, doc("d1", 1));
    assert_eq!(engine.stats().active_transactions, 1);
    engine.rollback(txn);
    assert_eq!(engine.stats().active_transactions, 0);
    assert!(engine.active_transactions().is_empty());
}

#[test]
fn transaction_unregisters_on_connection_close() {
    // A connection close is modelled by the server rolling back each open
    // transaction (see `Session::cleanup`). Transactions begun on a connection
    // carry its id; rolling them back releases their snapshots.
    let (_dir, engine, _clock) = open_with_clock(60);
    let txn_a = engine.begin_with_connection(Some(7));
    let txn_b = engine.begin_with_connection(Some(7));
    assert_eq!(engine.stats().active_transactions, 2);
    assert!(engine
        .active_transactions()
        .iter()
        .all(|t| t.connection_id == Some(7)));
    // Connection cleanup rolls back every transaction it owns.
    engine.rollback(txn_a);
    engine.rollback(txn_b);
    assert_eq!(engine.stats().active_transactions, 0);
}

#[test]
fn transaction_timeout_releases_snapshot() {
    let (_dir, engine, clock) = open_with_clock(60);
    let _txn = engine.begin();
    assert!(engine.stats().oldest_active_read_ts.is_some());
    clock.advance(120);
    assert_eq!(engine.reap_transactions(), 1);
    let stats = engine.stats();
    assert_eq!(stats.active_transactions, 0);
    assert_eq!(stats.timed_out_transactions, 1);
    // The pinned snapshot is released, so the GC horizon is no longer held back.
    assert_eq!(stats.oldest_active_read_ts, None);
}

#[test]
fn timed_out_transaction_rejects_operations() {
    let (_dir, engine, clock) = open_with_clock(60);
    let mut txn = engine.begin();
    clock.advance(120);
    engine.reap_transactions();

    let read_err = engine.txn_find(&txn, &FindQuery::new("Doc")).unwrap_err();
    assert!(matches!(read_err, Error::TransactionTimeout(_)));

    let stage_err = engine
        .stage(
            &mut txn,
            Mutation::Insert {
                collection: "Doc".into(),
                fields: doc("d1", 1),
            },
        )
        .unwrap_err();
    assert!(matches!(stage_err, Error::TransactionTimeout(_)));

    let commit_err = engine.commit(txn).unwrap_err();
    assert!(matches!(commit_err, Error::TransactionTimeout(_)));
}

#[test]
fn abandoned_transaction_reaper_releases_snapshot() {
    let (_dir, engine, clock) = open_with_clock(30);
    // A handle dropped without commit/rollback (an abandoned transaction) still
    // holds its registry entry — Drop cannot release it — so the reaper must.
    {
        let _txn = engine.begin();
    }
    assert_eq!(engine.stats().active_transactions, 1);
    clock.advance(45);
    assert_eq!(engine.reap_transactions(), 1);
    let stats = engine.stats();
    assert_eq!(stats.active_transactions, 0);
    assert_eq!(stats.oldest_active_read_ts, None);
}

#[test]
fn gc_progresses_after_timeout() {
    let (_dir, engine, clock) = open_with_clock(60);
    engine.insert("Doc", doc("d1", 1)).unwrap();
    // A long-lived transaction pins the snapshot that can see version 1.
    let txn = engine.begin();
    // A later committed update creates version 2.
    engine
        .apply_mutation(Mutation::Update {
            collection: "Doc".into(),
            filter: None,
            set: {
                let mut s = Document::new();
                s.insert("views".into(), Value::Int(2));
                s
            },
        })
        .unwrap();

    // With the transaction active, GC must preserve version 1 (it is visible at
    // the pinned snapshot), so nothing is reclaimed below the snapshot.
    let before = engine.gc().unwrap();
    assert_eq!(before.versions_reclaimed, 0);

    // After the transaction times out its snapshot is released, so GC can
    // reclaim the now-unreachable version 1.
    clock.advance(120);
    engine.reap_transactions();
    let after = engine.gc().unwrap();
    assert!(
        after.versions_reclaimed >= 1,
        "expected GC to reclaim versions after timeout, got {after:?}"
    );
    drop(txn);
}

#[test]
fn status_reports_active_transactions() {
    let (_dir, engine, clock) = open_with_clock(60);
    let _a = engine.begin_with_connection(Some(1));
    clock.advance(5);
    let _b = engine.begin_with_connection(Some(2));
    let stats = engine.stats();
    assert_eq!(stats.active_transactions, 2);
    assert!(stats.oldest_active_read_ts.is_some());
    // The oldest transaction's age reflects the first one begun.
    assert_eq!(stats.oldest_transaction_age_secs, Some(5));
    let actives = engine.active_transactions();
    assert_eq!(actives.len(), 2);
    assert!(actives.iter().all(|t| t.state == TxnState::Active));
    assert!(actives.iter().any(|t| t.connection_id == Some(1)));
}

#[test]
fn metrics_count_timed_out_transactions() {
    let (_dir, engine, clock) = open_with_clock(60);
    let _a = engine.begin();
    let _b = engine.begin();
    clock.advance(120);
    assert_eq!(engine.reap_transactions(), 2);
    assert_eq!(engine.stats().transaction_timeouts_total, 2);
    // A second reap with nothing newly idle does not double-count.
    assert_eq!(engine.reap_transactions(), 0);
    assert_eq!(engine.stats().transaction_timeouts_total, 2);
}

#[test]
fn disabled_timeout_never_reaps() {
    let (_dir, engine, clock) = open_with_clock(0);
    let _txn = engine.begin();
    clock.advance(100_000);
    assert_eq!(engine.reap_transactions(), 0);
    assert_eq!(engine.stats().active_transactions, 1);
}
