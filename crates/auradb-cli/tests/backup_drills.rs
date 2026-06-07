//! Backup and restore drills (release gate).
//!
//! Builds a mixed dataset, dumps it, verifies the dump with `backup verify`,
//! restores into a fresh directory, runs the structured consistency check, and
//! confirms that records, indexes, relationships, vectors, full-text, and
//! document-path data all survive. Also covers post-compaction backups and
//! rejection of corrupt input.

use std::path::Path;

use auradb::core::{
    Cardinality, CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, OnDelete,
    Relationship, Value,
};
use auradb::query::{CountQuery, Filter, FindQuery, VectorSearch};
use auradb::Engine;
use auradb_cli::{check_report, cmd_backup_verify, cmd_dump, cmd_restore};

const DIM: usize = 4;

/// Build a database exercising scalar, document, full-text, document-path,
/// vector, relationship, and indexed fields.
fn build_mixed(dir: &Path) {
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
    engine.analyze().unwrap();
    drop(engine);
}

/// Dump `src`, verify the dump, restore into a fresh dir, and return it.
fn dump_verify_restore(src: &Path) -> tempfile::TempDir {
    let dump_dir = tempfile::tempdir().unwrap();
    let dump = dump_dir.path().join("backup.jsonl");
    cmd_dump(src, &dump).unwrap();

    let (report, ok) = cmd_backup_verify(&dump).unwrap();
    assert!(ok, "verify must pass for a clean dump: {report}");

    let dst = tempfile::tempdir().unwrap();
    cmd_restore(dst.path(), &dump).unwrap();
    // Keep the dump dir alive for the duration by leaking it into the return —
    // instead we just let it drop; the restored dir is independent.
    drop(dump_dir);
    dst
}

#[test]
fn backup_restore_drill_mixed_dataset() {
    let src = tempfile::tempdir().unwrap();
    build_mixed(src.path());
    let dst = dump_verify_restore(src.path());

    let report = check_report(dst.path());
    assert!(
        report.ok,
        "restored dir must pass check: {:?}",
        report.errors
    );

    let engine = Engine::open(dst.path()).unwrap();
    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "Doc".into(),
                filter: None
            })
            .unwrap(),
        6
    );
    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "Person".into(),
                filter: None
            })
            .unwrap(),
        2
    );
}

#[test]
fn backup_restore_drill_indexes_and_stats() {
    let src = tempfile::tempdir().unwrap();
    build_mixed(src.path());
    let dst = dump_verify_restore(src.path());

    let report = check_report(dst.path());
    assert!(report.ok);
    assert_eq!(report.indexes.consistency_ok, Some(true));

    // Full-text and document-path indexes are usable after restore.
    let engine = Engine::open(dst.path()).unwrap();
    let ft = FindQuery {
        filter: Some(Filter::ContainsText {
            field: "body".into(),
            query: "fox".into(),
        }),
        ..FindQuery::new("Doc")
    };
    assert_eq!(engine.find(&ft).unwrap().len(), 6);
    let dp = FindQuery {
        filter: Some(Filter::Compare {
            field: "profile.company".into(),
            op: auradb::query::CompareOp::Eq,
            value: Value::Text("Acme0".into()),
        }),
        ..FindQuery::new("Doc")
    };
    assert!(!engine.find(&dp).unwrap().is_empty());
}

#[test]
fn backup_restore_drill_relationships() {
    let src = tempfile::tempdir().unwrap();
    build_mixed(src.path());
    let dst = dump_verify_restore(src.path());

    let engine = Engine::open(dst.path()).unwrap();
    let q = FindQuery {
        includes: vec!["owner".into()],
        ..FindQuery::new("Doc")
    };
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 6, "relationship include resolves after restore");
}

#[test]
fn backup_restore_drill_vectors() {
    let src = tempfile::tempdir().unwrap();
    build_mixed(src.path());
    let dst = dump_verify_restore(src.path());

    let engine = Engine::open(dst.path()).unwrap();
    let q = FindQuery {
        vector: Some(VectorSearch {
            field: "embedding".into(),
            query: vec![0.0; DIM],
            k: 3,
            metric: "euclidean".into(),
        }),
        ..FindQuery::new("Doc")
    };
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 3, "vector nearest returns k rows after restore");
}

#[test]
fn backup_restore_drill_after_compaction() {
    let src = tempfile::tempdir().unwrap();
    build_mixed(src.path());
    // Compact the source before dumping.
    {
        let engine = Engine::open(src.path()).unwrap();
        engine.compact().unwrap();
    }
    let dst = dump_verify_restore(src.path());
    let report = check_report(dst.path());
    assert!(report.ok, "restore after compaction is consistent");
    let engine = Engine::open(dst.path()).unwrap();
    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "Doc".into(),
                filter: None
            })
            .unwrap(),
        6
    );
}

#[test]
fn backup_restore_drill_jsonl_corruption_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("broken.jsonl");
    std::fs::write(
        &dump,
        "{\"type\":\"schema\",\"schema\":{\"name\":\"C\",\"fields\":[]}}\nnot-json-at-all\n",
    )
    .unwrap();

    // `backup verify` reports the malformed line as a fatal error.
    let (report, ok) = cmd_backup_verify(&dump).unwrap();
    assert!(!ok, "verify must reject a corrupt dump: {report}");

    // `restore` refuses the corrupt dump.
    let dst = tempfile::tempdir().unwrap();
    assert!(cmd_restore(dst.path(), &dump).is_err());
}
