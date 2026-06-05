//! MVCC garbage-collection correctness (v0.3.1).
//!
//! GC must never reclaim a version an active transaction can still observe, must
//! reclaim superseded versions once no reader can see them, must keep the latest
//! committed version of every live record, must keep a tombstone until no old
//! snapshot can see the pre-delete version, and must leave indexes and planner
//! statistics consistent. GC is idempotent and survives a restart.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{FindQuery, Mutation};
use auradb::storage::StorageOptions;
use auradb::{Engine, EngineOptions, WallClock};

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
        .with_field(FieldDef {
            name: "status".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: true,
            indexed: true,
        })
        .with_field(FieldDef::new("views", FieldType::Int))
}

fn doc(id: &str, views: i64) -> Document {
    let mut m = Document::new();
    m.insert("id".into(), Value::Text(id.into()));
    m.insert("status".into(), Value::Text("published".into()));
    m.insert("views".into(), Value::Int(views));
    m
}

fn open() -> (tempfile::TempDir, Engine, WallClock) {
    let dir = tempfile::tempdir().unwrap();
    let clock = WallClock::manual();
    let engine = Engine::open_with(
        dir.path(),
        EngineOptions {
            storage: StorageOptions::default(),
            gc_min_retained_versions: 1,
            transaction_timeout_secs: 60,
            clock: clock.clone(),
        },
    )
    .unwrap();
    engine.create_schema(schema()).unwrap();
    (dir, engine, clock)
}

fn update_views(engine: &Engine, views: i64) {
    let mut set = Document::new();
    set.insert("views".into(), Value::Int(views));
    engine
        .apply_mutation(Mutation::Update {
            collection: "Doc".into(),
            filter: None,
            set,
        })
        .unwrap();
}

#[test]
fn gc_idempotent() {
    let (_dir, engine, _clock) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    update_views(&engine, 2);
    update_views(&engine, 3);
    let first = engine.gc().unwrap();
    assert!(first.versions_reclaimed >= 1);
    // Running GC again at the same horizon reclaims nothing.
    let second = engine.gc().unwrap();
    assert_eq!(second.versions_reclaimed, 0);
    assert_eq!(second.records_removed, 0);
}

#[test]
fn gc_preserves_snapshot_reader_versions() {
    let (_dir, engine, _clock) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    let txn = engine.begin(); // snapshot can see views == 1
    update_views(&engine, 2);
    update_views(&engine, 3);

    // GC with the transaction active must not remove the version it can see.
    engine.gc().unwrap();
    let rows = engine.txn_find(&txn, &FindQuery::new("Doc")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields.get("views"), Some(&Value::Int(1)));
    // A non-transactional reader sees the latest version.
    let latest = engine.find(&FindQuery::new("Doc")).unwrap();
    assert_eq!(latest[0].fields.get("views"), Some(&Value::Int(3)));
    engine.rollback(txn);
}

#[test]
fn gc_removes_versions_after_snapshot_released() {
    let (_dir, engine, _clock) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    let txn = engine.begin();
    update_views(&engine, 2);
    update_views(&engine, 3);
    engine.rollback(txn); // release the snapshot

    let report = engine.gc().unwrap();
    assert!(
        report.versions_reclaimed >= 2,
        "expected superseded versions reclaimed, got {report:?}"
    );
    // Latest state is intact.
    let rows = engine.find(&FindQuery::new("Doc")).unwrap();
    assert_eq!(rows[0].fields.get("views"), Some(&Value::Int(3)));
}

#[test]
fn gc_preserves_delete_visibility_for_old_snapshot() {
    let (_dir, engine, _clock) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    let txn = engine.begin(); // snapshot sees d1 before deletion
    engine
        .apply_mutation(Mutation::Delete {
            collection: "Doc".into(),
            filter: None,
        })
        .unwrap();

    // GC must keep the pre-delete version while the old snapshot can see it.
    engine.gc().unwrap();
    assert_eq!(
        engine.txn_find(&txn, &FindQuery::new("Doc")).unwrap().len(),
        1
    );
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 0);

    // Once released, GC reclaims the tombstone chain entirely.
    engine.rollback(txn);
    let report = engine.gc().unwrap();
    assert!(report.records_removed >= 1, "{report:?}");
    assert_eq!(engine.stats().records, 0);
}

#[test]
fn gc_after_restart() {
    let (dir, engine, _clock) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    update_views(&engine, 2);
    engine.gc().unwrap();
    drop(engine);

    // Reopen and GC again: it runs cleanly and data is intact.
    let engine = Engine::open(dir.path()).unwrap();
    let report = engine.gc().unwrap();
    assert_eq!(report.versions_reclaimed, 0);
    let rows = engine.find(&FindQuery::new("Doc")).unwrap();
    assert_eq!(rows[0].fields.get("views"), Some(&Value::Int(2)));
}

#[test]
fn gc_preserves_index_consistency() {
    let (_dir, engine, _clock) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    engine.insert("Doc", doc("d2", 1)).unwrap();
    update_views(&engine, 9); // updates both
    engine
        .apply_mutation(Mutation::Delete {
            collection: "Doc".into(),
            filter: Some(auradb::query::Filter::Compare {
                field: "id".into(),
                op: auradb::query::CompareOp::Eq,
                value: Value::Text("d1".into()),
            }),
        })
        .unwrap();
    engine.gc().unwrap();
    // The secondary index on `status` must still agree with storage.
    let verified = engine.check_consistency().unwrap();
    assert!(verified >= 1);
}

#[test]
fn gc_preserves_planner_stats_or_recomputes_them() {
    let (_dir, engine, _clock) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    engine.insert("Doc", doc("d2", 1)).unwrap();
    engine.insert("Doc", doc("d3", 1)).unwrap();
    engine.analyze().unwrap();
    update_views(&engine, 5); // creates superseded versions
    engine.gc().unwrap();
    // GC does not corrupt planner stats: the row count still matches live data.
    let stats = engine.planner_stats();
    assert_eq!(stats.get("Doc").map(|c| c.row_count), Some(3));
    assert_eq!(engine.stats().records, 3);
}

#[test]
fn gc_reports_reclaimed_versions_and_bytes() {
    let (_dir, engine, _clock) = open();
    engine.insert("Doc", doc("d1", 0)).unwrap();
    for v in 1..=5 {
        update_views(&engine, v);
    }
    let report = engine.gc().unwrap();
    assert_eq!(report.versions_reclaimed, 5, "{report:?}");
    assert!(
        report.bytes_reclaimed > 0,
        "expected non-zero bytes reclaimed, got {report:?}"
    );
    assert_eq!(report.versions_after, 1);
}
