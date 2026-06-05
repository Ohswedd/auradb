//! Snapshot-isolation (MVCC) semantics.
//!
//! These tests pin a transaction's read timestamp at begin and verify that:
//! reads see a stable begin-time snapshot regardless of later commits; a
//! transaction sees its own writes; non-transactional reads see the latest
//! committed state; and commit rejects write-write / update-delete /
//! delete-update conflicts (first-committer-wins). Every snapshot-sensitive read
//! surface — point/scan, cursor, relationship include, vector, document-path,
//! full-text — is covered, plus version garbage collection.
//!
//! AuraDB v0.3.0 targets single-node snapshot isolation, not serializable
//! isolation; these tests assert exactly that contract and no more.

use auradb::core::{
    Cardinality, CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, OnDelete,
    Relationship, Value,
};
use auradb::query::{CompareOp, CountQuery, Filter, FindQuery, Mutation, VectorSearch};
use auradb::{Engine, Transaction};

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
        .with_field(FieldDef::new("metadata", FieldType::Document))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
        .with_field(FieldDef::new("body", FieldType::String))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
        .with_index(IndexDef {
            path: "metadata.source".into(),
            kind: IndexKind::DocumentPath,
        })
}

fn open() -> (tempfile::TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    (dir, engine)
}

fn doc(id: &str, views: i64) -> Document {
    let mut m = Document::new();
    m.insert("id".into(), Value::Text(id.into()));
    m.insert("status".into(), Value::Text("published".into()));
    m.insert("views".into(), Value::Int(views));
    m.insert("body".into(), Value::Text("alpha beta".into()));
    let mut meta = Document::new();
    meta.insert("source".into(), Value::Text("import".into()));
    m.insert("metadata".into(), Value::Object(meta));
    m
}

fn eq(field: &str, value: Value) -> Filter {
    Filter::Compare {
        field: field.into(),
        op: CompareOp::Eq,
        value,
    }
}

fn update(engine: &Engine, txn: &mut Transaction, id: &str, field: &str, value: Value) {
    let mut set = Document::new();
    set.insert(field.into(), value);
    engine
        .stage(
            txn,
            Mutation::Update {
                collection: "Doc".into(),
                filter: Some(eq("id", Value::Text(id.into()))),
                set,
            },
        )
        .unwrap();
}

fn delete(engine: &Engine, txn: &mut Transaction, id: &str) {
    engine
        .stage(
            txn,
            Mutation::Delete {
                collection: "Doc".into(),
                filter: Some(eq("id", Value::Text(id.into()))),
            },
        )
        .unwrap();
}

fn views_of(engine: &Engine, txn: &Transaction, id: &str) -> Option<i64> {
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("id", Value::Text(id.into())));
    let rows = engine.txn_find(txn, &q).unwrap();
    match rows.first()?.fields.get("views") {
        Some(Value::Int(v)) => Some(*v),
        _ => None,
    }
}

#[test]
fn snapshot_does_not_see_later_commit() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();

    let txn = engine.begin();
    // A concurrent auto-commit updates the record after the snapshot was pinned.
    engine
        .apply_mutation(Mutation::Update {
            collection: "Doc".into(),
            filter: Some(eq("id", Value::Text("d1".into()))),
            set: {
                let mut s = Document::new();
                s.insert("views".into(), Value::Int(2));
                s
            },
        })
        .unwrap();

    // The transaction still sees its begin-time snapshot.
    assert_eq!(views_of(&engine, &txn, "d1"), Some(1));
    let rows = engine.txn_find(&txn, &FindQuery::new("Doc")).unwrap();
    assert_eq!(rows[0].fields.get("views"), Some(&Value::Int(1)));

    // A non-transactional reader sees the latest committed state.
    let latest = engine.find(&FindQuery::new("Doc")).unwrap();
    assert_eq!(latest[0].fields.get("views"), Some(&Value::Int(2)));
}

#[test]
fn transaction_sees_own_insert_update_and_hides_delete() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("base", 1)).unwrap();
    let mut txn = engine.begin();

    // Own insert.
    engine
        .stage(
            &mut txn,
            Mutation::Insert {
                collection: "Doc".into(),
                fields: doc("fresh", 7),
            },
        )
        .unwrap();
    assert_eq!(views_of(&engine, &txn, "fresh"), Some(7));

    // Own update.
    update(&engine, &mut txn, "base", "views", Value::Int(99));
    assert_eq!(views_of(&engine, &txn, "base"), Some(99));

    // Own delete is hidden from the transaction.
    delete(&engine, &mut txn, "base");
    assert_eq!(views_of(&engine, &txn, "base"), None);
}

#[test]
fn non_transactional_read_sees_latest() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    let _holding = engine.begin(); // an open snapshot must not pin latest reads
    engine
        .apply_mutation(Mutation::Update {
            collection: "Doc".into(),
            filter: Some(eq("id", Value::Text("d1".into()))),
            set: {
                let mut s = Document::new();
                s.insert("views".into(), Value::Int(5));
                s
            },
        })
        .unwrap();
    let rows = engine.find(&FindQuery::new("Doc")).unwrap();
    assert_eq!(rows[0].fields.get("views"), Some(&Value::Int(5)));
}

#[test]
fn write_write_conflict_rejected() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();

    let mut a = engine.begin();
    let mut b = engine.begin();
    update(&engine, &mut a, "d1", "views", Value::Int(10));
    update(&engine, &mut b, "d1", "views", Value::Int(20));

    // First committer wins; the second sees a conflict.
    engine.commit(a).unwrap();
    let err = engine.commit(b).unwrap_err();
    assert!(matches!(err, auradb::core::Error::Conflict(_)), "{err:?}");

    // The winner's value is what stuck.
    let rows = engine.find(&FindQuery::new("Doc")).unwrap();
    assert_eq!(rows[0].fields.get("views"), Some(&Value::Int(10)));
}

#[test]
fn update_delete_conflict_rejected() {
    // Winner updates, loser deletes the same record.
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    let mut a = engine.begin();
    let mut b = engine.begin();
    update(&engine, &mut a, "d1", "views", Value::Int(2));
    delete(&engine, &mut b, "d1");
    engine.commit(a).unwrap();
    assert!(matches!(
        engine.commit(b).unwrap_err(),
        auradb::core::Error::Conflict(_)
    ));
}

#[test]
fn delete_update_conflict_rejected() {
    // Winner deletes, loser updates the same record.
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    let mut a = engine.begin();
    let mut b = engine.begin();
    delete(&engine, &mut a, "d1");
    update(&engine, &mut b, "d1", "views", Value::Int(2));
    engine.commit(a).unwrap();
    assert!(matches!(
        engine.commit(b).unwrap_err(),
        auradb::core::Error::Conflict(_)
    ));
}

#[test]
fn rollback_discards_versions() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    let versions_before = engine.stats().versions;

    let mut txn = engine.begin();
    update(&engine, &mut txn, "d1", "views", Value::Int(2));
    engine.rollback(txn);

    // No new version was written, and the rolled-back snapshot is released.
    assert_eq!(engine.stats().versions, versions_before);
    assert_eq!(engine.stats().active_transactions, 0);
    let rows = engine.find(&FindQuery::new("Doc")).unwrap();
    assert_eq!(rows[0].fields.get("views"), Some(&Value::Int(1)));
}

#[test]
fn commit_assigns_monotonic_commit_ts() {
    let (_dir, engine) = open();
    let t1 = engine.begin();
    let r1 = t1.read_ts();
    engine.rollback(t1);

    engine.insert("Doc", doc("d1", 1)).unwrap();
    let t2 = engine.begin();
    let r2 = t2.read_ts();
    engine.rollback(t2);

    engine.insert("Doc", doc("d2", 1)).unwrap();
    let t3 = engine.begin();
    let r3 = t3.read_ts();
    engine.rollback(t3);

    // Each commit advances the watermark, so successive snapshots are monotone.
    assert!(r2 > r1, "{r2} !> {r1}");
    assert!(r3 > r2, "{r3} !> {r2}");
}

#[test]
fn concurrent_readers_keep_snapshot() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();
    let a = engine.begin();
    let b = engine.begin();
    engine
        .apply_mutation(Mutation::Update {
            collection: "Doc".into(),
            filter: Some(eq("id", Value::Text("d1".into()))),
            set: {
                let mut s = Document::new();
                s.insert("views".into(), Value::Int(2));
                s
            },
        })
        .unwrap();
    assert_eq!(views_of(&engine, &a, "d1"), Some(1));
    assert_eq!(views_of(&engine, &b, "d1"), Some(1));
    assert_eq!(engine.stats().active_transactions, 2);
}

#[test]
fn cursor_keeps_snapshot_after_later_commit() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();

    let txn = engine.begin();
    // Open a cursor (plan) at the snapshot.
    let planned = engine.txn_plan_find(&txn, &FindQuery::new("Doc")).unwrap();
    assert_eq!(planned.ordered.len(), 1);

    // A later commit inserts a new record.
    engine.insert("Doc", doc("d2", 1)).unwrap();

    // Paging the cursor and re-reading still reflect the pinned snapshot.
    let page = engine
        .txn_materialize(&txn, &FindQuery::new("Doc"), &planned.ordered)
        .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(
        engine.txn_find(&txn, &FindQuery::new("Doc")).unwrap().len(),
        1
    );
    // Latest readers see both.
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 2);
}

#[test]
fn vector_nearest_uses_snapshot() {
    let (_dir, engine) = open();
    let mut d = doc("d1", 1);
    d.insert("embedding".into(), Value::Vector(vec![1.0, 0.0, 0.0]));
    engine.insert("Doc", d).unwrap();

    let txn = engine.begin();
    // A second vector record is committed after the snapshot.
    let mut d2 = doc("d2", 1);
    d2.insert("embedding".into(), Value::Vector(vec![1.0, 0.0, 0.0]));
    engine.insert("Doc", d2).unwrap();

    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![1.0, 0.0, 0.0],
        k: 10,
        metric: "cosine".into(),
    });
    // The transaction's vector search only sees the snapshot record.
    assert_eq!(engine.txn_find(&txn, &q).unwrap().len(), 1);
    // Latest sees both.
    assert_eq!(engine.find(&q).unwrap().len(), 2);
}

#[test]
fn document_path_index_uses_snapshot() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap(); // metadata.source = "import"

    let txn = engine.begin();
    // Commit a record with a different document-path value after the snapshot.
    let mut d2 = doc("d2", 1);
    let mut meta = Document::new();
    meta.insert("source".into(), Value::Text("later".into()));
    d2.insert("metadata".into(), Value::Object(meta));
    engine.insert("Doc", d2).unwrap();

    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("metadata.source", Value::Text("later".into())));
    // Not visible in the snapshot...
    assert_eq!(engine.txn_find(&txn, &q).unwrap().len(), 0);
    // ...but visible to latest readers.
    assert_eq!(engine.find(&q).unwrap().len(), 1);
}

#[test]
fn full_text_uses_snapshot() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap(); // body = "alpha beta"

    let txn = engine.begin();
    let mut d2 = doc("d2", 1);
    d2.insert("body".into(), Value::Text("gamma delta".into()));
    engine.insert("Doc", d2).unwrap();

    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::ContainsText {
        field: "body".into(),
        query: "gamma".into(),
    });
    assert_eq!(engine.txn_find(&txn, &q).unwrap().len(), 0);
    assert_eq!(engine.find(&q).unwrap().len(), 1);
}

#[test]
fn relationship_include_uses_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    // Workspace target collection.
    engine
        .create_schema(
            CollectionSchema::new("Workspace")
                .with_field(FieldDef {
                    name: "id".into(),
                    field_type: FieldType::String,
                    primary_key: true,
                    unique: true,
                    nullable: false,
                    indexed: false,
                })
                .with_field(FieldDef::new("name", FieldType::String)),
        )
        .unwrap();
    // Item collection referencing a workspace.
    engine
        .create_schema(
            CollectionSchema::new("Item")
                .with_field(FieldDef {
                    name: "id".into(),
                    field_type: FieldType::String,
                    primary_key: true,
                    unique: true,
                    nullable: false,
                    indexed: false,
                })
                .with_relationship(Relationship {
                    name: "workspace".into(),
                    target: "Workspace".into(),
                    cardinality: Cardinality::ToOne,
                    on_delete: OnDelete::Restrict,
                }),
        )
        .unwrap();

    let mut ws = Document::new();
    ws.insert("id".into(), Value::Text("w1".into()));
    ws.insert("name".into(), Value::Text("original".into()));
    engine.insert("Workspace", ws).unwrap();
    let mut item = Document::new();
    item.insert("id".into(), Value::Text("i1".into()));
    item.insert("workspace".into(), Value::Text("w1".into()));
    engine.insert("Item", item).unwrap();

    let txn = engine.begin();
    // Mutate the related workspace after the snapshot.
    engine
        .apply_mutation(Mutation::Update {
            collection: "Workspace".into(),
            filter: Some(eq("id", Value::Text("w1".into()))),
            set: {
                let mut s = Document::new();
                s.insert("name".into(), Value::Text("renamed".into()));
                s
            },
        })
        .unwrap();

    let mut q = FindQuery::new("Item");
    q.includes = vec!["workspace".into()];
    let rows = engine.txn_find(&txn, &q).unwrap();
    let included = &rows[0].includes["workspace"][0];
    // The included related record reflects the snapshot, not the later rename.
    assert_eq!(included.get("name"), Some(&Value::Text("original".into())));
}

#[test]
fn gc_reclaims_old_versions_but_keeps_active_snapshot() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1", 1)).unwrap();

    // An open transaction pins the old version.
    let holder = engine.begin();
    engine
        .apply_mutation(Mutation::Update {
            collection: "Doc".into(),
            filter: Some(eq("id", Value::Text("d1".into()))),
            set: {
                let mut s = Document::new();
                s.insert("views".into(), Value::Int(2));
                s
            },
        })
        .unwrap();

    // GC cannot reclaim the old version while the snapshot is held.
    let report = engine.gc().unwrap();
    assert_eq!(report.versions_reclaimed, 0);
    assert_eq!(views_of(&engine, &holder, "d1"), Some(1));

    // After the holder finishes, GC reclaims the superseded version.
    engine.rollback(holder);
    let report = engine.gc().unwrap();
    assert_eq!(report.versions_reclaimed, 1);
    // Latest value is preserved.
    let rows = engine.find(&FindQuery::new("Doc")).unwrap();
    assert_eq!(rows[0].fields.get("views"), Some(&Value::Int(2)));
    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "Doc".into(),
                filter: None
            })
            .unwrap(),
        1
    );
}
