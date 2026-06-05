//! Backup and restore verification.
//!
//! Builds a database that exercises every field kind and index kind, runs it
//! through `auradb dump` and `auradb restore`, reopens the restored directory,
//! and verifies that records, schema, indexes, and every search path survived.

use auradb::core::{
    Cardinality, CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, OnDelete,
    Relationship, Value,
};
use auradb::query::{CompareOp, CountQuery, ExistsQuery, Filter, FindQuery, VectorSearch};
use auradb::Engine;
use auradb_cli::{cmd_check, cmd_dump, cmd_restore};

const DIM: usize = 4;

/// Build the source database with scalar, document, vector, relationship,
/// full-text, and document-path data.
fn build_source(dir: &std::path::Path) {
    let engine = Engine::open(dir).unwrap();

    let person = CollectionSchema::new("Person").with_field(FieldDef {
        name: "id".into(),
        field_type: FieldType::Uuid,
        primary_key: true,
        unique: true,
        nullable: false,
        indexed: false,
    });
    engine.create_schema(person).unwrap();

    let doc = CollectionSchema::new("Doc")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef {
            name: "title".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: false,
            indexed: true,
        })
        .with_field(FieldDef::new("count", FieldType::Int))
        .with_field(FieldDef::new("profile", FieldType::Document))
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: DIM }))
        .with_relationship(Relationship {
            name: "owner".into(),
            target: "Person".into(),
            cardinality: Cardinality::ToOne,
            on_delete: OnDelete::Restrict,
        })
        .with_index(IndexDef {
            path: "profile.company".into(),
            kind: IndexKind::DocumentPath,
        })
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        });
    engine.create_schema(doc).unwrap();

    for i in 0..2 {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("person{i}")));
        engine.insert("Person", f).unwrap();
    }

    for i in 0..6 {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("doc{i}")));
        f.insert("title".into(), Value::Text(format!("Title {i}")));
        f.insert("count".into(), Value::Int(i as i64));
        let mut profile = Document::new();
        profile.insert("company".into(), Value::Text(format!("Acme{}", i % 2)));
        f.insert("profile".into(), Value::Object(profile));
        f.insert(
            "body".into(),
            Value::Text(format!("the quick brown fox number {i} jumps")),
        );
        let v: Vec<f32> = (0..DIM).map(|j| (i + j) as f32).collect();
        f.insert("embedding".into(), Value::Vector(v));
        f.insert("owner".into(), Value::Text(format!("person{}", i % 2)));
        engine.insert("Doc", f).unwrap();
    }
}

#[test]
fn dump_restore_preserves_all_field_and_index_kinds() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dump = tmp.path().join("backup.jsonl");
    let dst = tmp.path().join("restored");

    build_source(&src);

    // Dump and restore through the CLI command implementations.
    let lines = cmd_dump(&src, &dump).unwrap();
    // 2 schemas + 2 Person + 6 Doc records.
    assert_eq!(lines, 2 + 2 + 6);
    let restored = cmd_restore(&dst, &dump).unwrap();
    assert_eq!(restored, 8);

    // Reopen the restored directory and verify everything survived.
    let engine = Engine::open(&dst).unwrap();

    // Schema exists.
    let names: Vec<String> = engine.list_schemas().into_iter().map(|s| s.name).collect();
    assert!(names.contains(&"Doc".to_string()));
    assert!(names.contains(&"Person".to_string()));

    // Records exist.
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 6);

    // Count and exists.
    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "Doc".into(),
                filter: None,
            })
            .unwrap(),
        6
    );
    assert!(engine
        .exists(&ExistsQuery {
            collection: "Doc".into(),
            filter: Some(Filter::Compare {
                field: "id".into(),
                op: CompareOp::Eq,
                value: Value::Text("doc3".into()),
            }),
        })
        .unwrap());

    // Secondary index lookup.
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::Compare {
        field: "title".into(),
        op: CompareOp::Eq,
        value: Value::Text("Title 2".into()),
    });
    assert_eq!(engine.find(&q).unwrap().len(), 1);

    // Document-path index lookup.
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::Compare {
        field: "profile.company".into(),
        op: CompareOp::Eq,
        value: Value::Text("Acme0".into()),
    });
    assert_eq!(engine.find(&q).unwrap().len(), 3);

    // Full-text search.
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::ContainsText {
        field: "body".into(),
        query: "quick fox".into(),
    });
    assert_eq!(engine.find(&q).unwrap().len(), 6);

    // Vector nearest search.
    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![0.0; DIM],
        k: 3,
        metric: "euclidean".into(),
    });
    let near = engine.find(&q).unwrap();
    assert_eq!(near.len(), 3);
    assert!(near[0].score.is_some());

    // Relationship include hydrates the linked Person.
    let mut q = FindQuery::new("Doc");
    q.includes = vec!["owner".into()];
    q.filter = Some(Filter::Compare {
        field: "id".into(),
        op: CompareOp::Eq,
        value: Value::Text("doc0".into()),
    });
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].includes.contains_key("owner"),
        "relationship include should hydrate the owner"
    );

    // `auradb check` passes on the restored database.
    assert!(cmd_check(&dst).unwrap().contains("OK"));
}

// ---- Backup / restore combined with MVCC garbage collection (v0.3.1) ----
//
// AuraDB's logical dump exports the latest committed visible state, not the full
// version history. These tests pin that contract down against GC: a dump after
// GC restores the same visible state, restoring then GC'ing is safe, a dump
// taken while a snapshot is held is still the latest committed state, and a
// restore never resurrects versions GC has reclaimed.

/// A minimal single-collection database for the GC interaction tests.
fn build_versioned(dir: &std::path::Path) {
    let engine = Engine::open(dir).unwrap();
    let schema = CollectionSchema::new("Item")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("views", FieldType::Int));
    engine.create_schema(schema).unwrap();
    for i in 0..4 {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("item{i}")));
        f.insert("views".into(), Value::Int(0));
        engine.insert("Item", f).unwrap();
    }
    // Create superseded versions by updating every item twice.
    for v in 1..=2 {
        engine
            .apply_mutation(auradb::query::Mutation::Update {
                collection: "Item".into(),
                filter: None,
                set: {
                    let mut s = Document::new();
                    s.insert("views".into(), Value::Int(v));
                    s
                },
            })
            .unwrap();
    }
}

#[test]
fn backup_after_gc_restores_visible_latest_state() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dump = tmp.path().join("backup.jsonl");
    let dst = tmp.path().join("restored");
    build_versioned(&src);

    // GC reclaims superseded versions, then we back up the latest state.
    let engine = Engine::open(&src).unwrap();
    let report = engine.gc().unwrap();
    assert!(report.versions_reclaimed > 0);
    drop(engine);

    cmd_dump(&src, &dump).unwrap();
    cmd_restore(&dst, &dump).unwrap();
    let restored = Engine::open(&dst).unwrap();
    let rows = restored.find(&FindQuery::new("Item")).unwrap();
    assert_eq!(rows.len(), 4);
    for r in &rows {
        assert_eq!(r.fields.get("views"), Some(&Value::Int(2)));
    }
}

#[test]
fn restore_then_gc_is_safe() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dump = tmp.path().join("backup.jsonl");
    let dst = tmp.path().join("restored");
    build_versioned(&src);
    cmd_dump(&src, &dump).unwrap();
    cmd_restore(&dst, &dump).unwrap();

    let engine = Engine::open(&dst).unwrap();
    engine.gc().unwrap();
    engine.gc().unwrap(); // idempotent
    assert_eq!(engine.find(&FindQuery::new("Item")).unwrap().len(), 4);
    assert!(cmd_check(&dst).unwrap().contains("OK"));
}

#[test]
fn backup_with_active_snapshot_is_consistent() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dump = tmp.path().join("backup.jsonl");
    let dst = tmp.path().join("restored");
    build_versioned(&src);

    let engine = Engine::open(&src).unwrap();
    // Hold a snapshot while the backup runs; the logical dump still exports the
    // latest committed state, not the snapshot's older view.
    let txn = engine.begin();
    let lines = cmd_dump(&src, &dump).unwrap();
    assert_eq!(lines, 1 + 4); // schema + 4 records
    engine.rollback(txn);

    cmd_restore(&dst, &dump).unwrap();
    let restored = Engine::open(&dst).unwrap();
    let rows = restored.find(&FindQuery::new("Item")).unwrap();
    assert_eq!(rows.len(), 4);
    for r in &rows {
        assert_eq!(r.fields.get("views"), Some(&Value::Int(2)));
    }
}

#[test]
fn dump_restore_preserves_mvcc_latest_state() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dump = tmp.path().join("backup.jsonl");
    let dst = tmp.path().join("restored");
    build_versioned(&src);

    cmd_dump(&src, &dump).unwrap();
    cmd_restore(&dst, &dump).unwrap();
    let restored = Engine::open(&dst).unwrap();
    // Latest value preserved; superseded versions are not exported.
    let rows = restored.find(&FindQuery::new("Item")).unwrap();
    assert!(rows
        .iter()
        .all(|r| r.fields.get("views") == Some(&Value::Int(2))));
    // The restored store starts with one version per record (a fresh insert).
    assert_eq!(restored.stats().records, 4);
    assert_eq!(restored.stats().versions, 4);
}

#[test]
fn dump_restore_does_not_resurrect_gc_removed_versions() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dump = tmp.path().join("backup.jsonl");
    let dst = tmp.path().join("restored");
    build_versioned(&src);

    let engine = Engine::open(&src).unwrap();
    engine.gc().unwrap(); // reclaim old versions in the source
    drop(engine);

    cmd_dump(&src, &dump).unwrap();
    cmd_restore(&dst, &dump).unwrap();
    let restored = Engine::open(&dst).unwrap();
    // No reclaimed version reappears: exactly one version per live record.
    assert_eq!(restored.stats().versions, 4);
    let rows = restored.find(&FindQuery::new("Item")).unwrap();
    assert!(rows
        .iter()
        .all(|r| r.fields.get("views") == Some(&Value::Int(2))));
}
