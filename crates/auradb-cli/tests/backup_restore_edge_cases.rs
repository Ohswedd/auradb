//! Backup and restore edge cases (v0.8.1).
//!
//! Complements `backup_restore.rs` and `backup_drills.rs` with the awkward
//! corners: an empty database, a schema-only export, a large single record, a
//! record full of Unicode and escaped characters, deeply nested documents,
//! vectors, relationship delete policies, full-text with punctuation, and
//! document-path indexes after restore. The second half pins down the rejection
//! contract: malformed lines, records for an undeclared collection, duplicate
//! primary keys, truncated files, invalid schema sections, the per-line size
//! bound, and that `backup verify` never echoes record contents.

use auradb::core::{
    Cardinality, CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, OnDelete,
    Relationship, Value,
};
use auradb::query::{CompareOp, Filter, FindQuery};
use auradb::Engine;
use auradb_cli::{check_report, cmd_backup_verify, cmd_dump, cmd_restore, MAX_RESTORE_LINE_BYTES};

/// A schema with a UUID primary key named `id`.
fn pk(name: &str) -> CollectionSchema {
    CollectionSchema::new(name).with_field(FieldDef {
        name: "id".into(),
        field_type: FieldType::Uuid,
        primary_key: true,
        unique: true,
        nullable: false,
        indexed: false,
    })
}

/// Dump `src`, assert verify passes, restore into a fresh dir, and return it.
fn round_trip(src: &std::path::Path) -> tempfile::TempDir {
    let dump_dir = tempfile::tempdir().unwrap();
    let dump = dump_dir.path().join("backup.jsonl");
    cmd_dump(src, &dump).unwrap();
    let (report, ok) = cmd_backup_verify(&dump).unwrap();
    assert!(ok, "verify must pass for a clean dump: {report}");
    let dst = tempfile::tempdir().unwrap();
    cmd_restore(dst.path(), &dump).unwrap();
    dst
}

#[test]
fn backup_restore_empty_database() {
    let src = tempfile::tempdir().unwrap();
    Engine::open(src.path()).unwrap(); // create an empty data dir, no schemas
    let dump_dir = tempfile::tempdir().unwrap();
    let dump = dump_dir.path().join("backup.jsonl");
    let lines = cmd_dump(src.path(), &dump).unwrap();
    assert_eq!(lines, 0, "an empty database dumps zero lines");

    let (_report, ok) = cmd_backup_verify(&dump).unwrap();
    assert!(ok, "an empty dump verifies clean");

    let dst = tempfile::tempdir().unwrap();
    let restored = cmd_restore(dst.path(), &dump).unwrap();
    assert_eq!(restored, 0);
    assert!(check_report(dst.path()).ok);
}

#[test]
fn backup_restore_schema_only() {
    let src = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(src.path()).unwrap();
        engine.create_schema(pk("Person")).unwrap();
        engine.create_schema(pk("Org")).unwrap();
    }
    let dump_dir = tempfile::tempdir().unwrap();
    let dump = dump_dir.path().join("backup.jsonl");
    let lines = cmd_dump(src.path(), &dump).unwrap();
    assert_eq!(lines, 2, "two schemas, no records");

    let dst = round_trip(src.path());
    let engine = Engine::open(dst.path()).unwrap();
    let names: Vec<String> = engine.list_schemas().into_iter().map(|s| s.name).collect();
    assert!(names.contains(&"Person".to_string()));
    assert!(names.contains(&"Org".to_string()));
    assert_eq!(engine.find(&FindQuery::new("Person")).unwrap().len(), 0);
}

#[test]
fn backup_restore_large_record_near_line_limit() {
    let src = tempfile::tempdir().unwrap();
    // A single record with a ~512 KiB string field: far larger than a normal
    // row, but well under the per-line restore bound.
    let big = "x".repeat(512 * 1024);
    {
        let engine = Engine::open(src.path()).unwrap();
        engine
            .create_schema(pk("Big").with_field(FieldDef::new("blob", FieldType::String)))
            .unwrap();
        let mut f = Document::new();
        f.insert("id".into(), Value::Text("big0".into()));
        f.insert("blob".into(), Value::Text(big.clone()));
        engine.insert("Big", f).unwrap();
    }
    let dst = round_trip(src.path());
    let engine = Engine::open(dst.path()).unwrap();
    let rows = engine.find(&FindQuery::new("Big")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields.get("blob"), Some(&Value::Text(big)));
}

#[test]
fn backup_restore_unicode_and_escaped_strings() {
    let src = tempfile::tempdir().unwrap();
    // Mixed scripts, emoji, quotes, backslashes, newlines, tabs, and a NUL.
    let tricky = "héllo \"wörld\"\n\t\\path\\to — 日本語 🚀 \u{0}end";
    {
        let engine = Engine::open(src.path()).unwrap();
        engine
            .create_schema(pk("U").with_field(FieldDef::new("s", FieldType::String)))
            .unwrap();
        let mut f = Document::new();
        f.insert("id".into(), Value::Text("u0".into()));
        f.insert("s".into(), Value::Text(tricky.into()));
        engine.insert("U", f).unwrap();
    }
    let dst = round_trip(src.path());
    let engine = Engine::open(dst.path()).unwrap();
    let rows = engine.find(&FindQuery::new("U")).unwrap();
    assert_eq!(rows[0].fields.get("s"), Some(&Value::Text(tricky.into())));
}

#[test]
fn backup_restore_nested_documents_near_depth_limit() {
    let src = tempfile::tempdir().unwrap();
    // Build { id, profile: { a: { b: { c: { d: "leaf" } } } } }.
    let mut node = Document::new();
    node.insert("d".into(), Value::Text("leaf".into()));
    for key in ["c", "b", "a"] {
        let mut parent = Document::new();
        parent.insert(key.into(), Value::Object(node));
        node = parent;
    }
    {
        let engine = Engine::open(src.path()).unwrap();
        engine
            .create_schema(pk("N").with_field(FieldDef::new("profile", FieldType::Document)))
            .unwrap();
        let mut f = Document::new();
        f.insert("id".into(), Value::Text("n0".into()));
        f.insert("profile".into(), Value::Object(node.clone()));
        engine.insert("N", f).unwrap();
    }
    let dst = round_trip(src.path());
    let engine = Engine::open(dst.path()).unwrap();
    let rows = engine.find(&FindQuery::new("N")).unwrap();
    assert_eq!(rows[0].fields.get("profile"), Some(&Value::Object(node)));
}

#[test]
fn backup_restore_vectors_round_trip() {
    let src = tempfile::tempdir().unwrap();
    let v: Vec<f32> = vec![0.5, -1.25, 3.0, 42.0, -0.0];
    {
        let engine = Engine::open(src.path()).unwrap();
        engine
            .create_schema(pk("V").with_field(FieldDef::new("e", FieldType::Vector { dim: 5 })))
            .unwrap();
        let mut f = Document::new();
        f.insert("id".into(), Value::Text("v0".into()));
        f.insert("e".into(), Value::Vector(v.clone()));
        engine.insert("V", f).unwrap();
    }
    let dst = round_trip(src.path());
    let engine = Engine::open(dst.path()).unwrap();
    let rows = engine.find(&FindQuery::new("V")).unwrap();
    assert_eq!(rows[0].fields.get("e"), Some(&Value::Vector(v)));
}

#[test]
fn backup_restore_relationship_delete_policy_round_trip() {
    let src = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(src.path()).unwrap();
        engine.create_schema(pk("Person")).unwrap();
        let doc = pk("Doc").with_relationship(Relationship {
            name: "owner".into(),
            target: "Person".into(),
            cardinality: Cardinality::ToOne,
            on_delete: OnDelete::SetNull,
        });
        engine.create_schema(doc).unwrap();
    }
    let dst = round_trip(src.path());
    let engine = Engine::open(dst.path()).unwrap();
    let doc = engine
        .list_schemas()
        .into_iter()
        .find(|s| s.name == "Doc")
        .unwrap();
    let rel = doc
        .relationships
        .iter()
        .find(|r| r.name == "owner")
        .unwrap();
    assert_eq!(
        rel.on_delete,
        OnDelete::SetNull,
        "the delete policy survives a backup round trip"
    );
}

#[test]
fn backup_restore_full_text_punctuation_case() {
    let src = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(src.path()).unwrap();
        engine
            .create_schema(
                pk("Note")
                    .with_field(FieldDef::new("body", FieldType::String))
                    .with_index(IndexDef {
                        path: "body".into(),
                        kind: IndexKind::FullText,
                    }),
            )
            .unwrap();
        let mut f = Document::new();
        f.insert("id".into(), Value::Text("note0".into()));
        f.insert(
            "body".into(),
            Value::Text("Hello, WORLD! Rust-native; full_text? Yes.".into()),
        );
        engine.insert("Note", f).unwrap();
    }
    let dst = round_trip(src.path());
    let engine = Engine::open(dst.path()).unwrap();
    // Case-insensitive token match survives punctuation in both query and corpus.
    let q = FindQuery {
        filter: Some(Filter::ContainsText {
            field: "body".into(),
            query: "world rust".into(),
        }),
        ..FindQuery::new("Note")
    };
    assert_eq!(engine.find(&q).unwrap().len(), 1);
}

#[test]
fn backup_restore_document_path_indexes_after_restore() {
    let src = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(src.path()).unwrap();
        engine
            .create_schema(
                pk("Doc")
                    .with_field(FieldDef::new("profile", FieldType::Document))
                    .with_index(IndexDef {
                        path: "profile.company".into(),
                        kind: IndexKind::DocumentPath,
                    }),
            )
            .unwrap();
        for i in 0..4 {
            let mut profile = Document::new();
            profile.insert("company".into(), Value::Text(format!("Acme{}", i % 2)));
            let mut f = Document::new();
            f.insert("id".into(), Value::Text(format!("d{i}")));
            f.insert("profile".into(), Value::Object(profile));
            engine.insert("Doc", f).unwrap();
        }
    }
    let dst = round_trip(src.path());
    let report = check_report(dst.path());
    assert_eq!(report.indexes.consistency_ok, Some(true));

    let engine = Engine::open(dst.path()).unwrap();
    let q = FindQuery {
        filter: Some(Filter::Compare {
            field: "profile.company".into(),
            op: CompareOp::Eq,
            value: Value::Text("Acme0".into()),
        }),
        ..FindQuery::new("Doc")
    };
    assert_eq!(engine.find(&q).unwrap().len(), 2);
}

// ---- Rejection contract ---------------------------------------------------

#[test]
fn restore_rejects_malformed_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("bad.jsonl");
    std::fs::write(
        &dump,
        "{\"type\":\"schema\",\"schema\":{\"name\":\"C\",\"fields\":[]}}\n}{not json\n",
    )
    .unwrap();
    let dst = tempfile::tempdir().unwrap();
    assert!(
        cmd_restore(dst.path(), &dump).is_err(),
        "restore must refuse a malformed line"
    );
}

#[test]
fn restore_rejects_unknown_collection() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("ghost.jsonl");
    // A record line for a collection that no schema line declares.
    std::fs::write(
        &dump,
        "{\"type\":\"record\",\"collection\":\"Ghost\",\"fields\":{\"id\":\"x\"}}\n",
    )
    .unwrap();
    let dst = tempfile::tempdir().unwrap();
    let err = cmd_restore(dst.path(), &dump).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("ghost")
            || err.to_string().to_lowercase().contains("collection"),
        "error should identify the missing collection: {err}"
    );
}

#[test]
fn backup_verify_rejects_duplicate_primary_key() {
    let src = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(src.path()).unwrap();
        engine.create_schema(pk("C")).unwrap();
        let mut f = Document::new();
        f.insert("id".into(), Value::Text("pkValue7788".into()));
        engine.insert("C", f).unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dupe.jsonl");
    // Produce a faithful dump (schema + one record), then append a second record
    // line that reuses the same primary key — exactly what a corrupt or
    // hand-edited backup looks like.
    cmd_dump(src.path(), &dump).unwrap();
    let mut contents = std::fs::read_to_string(&dump).unwrap();
    let dup_line = contents
        .lines()
        .find(|l| l.contains("\"record\""))
        .unwrap()
        .to_string();
    contents.push_str(&dup_line);
    contents.push('\n');
    std::fs::write(&dump, contents).unwrap();

    let (report, ok) = cmd_backup_verify(&dump).unwrap();
    assert!(!ok, "duplicate primary key must fail verify: {report}");
    assert!(report.contains("duplicate primary key"), "{report}");
    // The collection is named but the duplicated key value itself is never printed.
    assert!(report.contains("`C`"));
    assert!(
        !report.contains("pkValue7788"),
        "verify must not echo the key value: {report}"
    );
}

#[test]
fn backup_verify_rejects_truncated_file() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("truncated.jsonl");
    // A valid schema line followed by a record line cut off mid-JSON.
    std::fs::write(
        &dump,
        "{\"type\":\"schema\",\"schema\":{\"name\":\"C\",\"fields\":[]}}\n{\"type\":\"record\",\"collection\":\"C\",\"fields\":{\"id\":\"x",
    )
    .unwrap();
    let (report, ok) = cmd_backup_verify(&dump).unwrap();
    assert!(!ok, "a truncated dump must fail verify: {report}");
}

#[test]
fn backup_verify_rejects_invalid_schema() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("badschema.jsonl");
    // `fields` is a string, not an array of field definitions.
    std::fs::write(
        &dump,
        "{\"type\":\"schema\",\"schema\":{\"name\":\"C\",\"fields\":\"oops\"}}\n",
    )
    .unwrap();
    let (report, ok) = cmd_backup_verify(&dump).unwrap();
    assert!(!ok, "an invalid schema section must fail verify: {report}");
}

#[test]
fn backup_verify_redacts_secrets() {
    let src = tempfile::tempdir().unwrap();
    const SECRET: &str = "TOP_SECRET_api_key_9f3c";
    {
        let engine = Engine::open(src.path()).unwrap();
        engine
            .create_schema(pk("S").with_field(FieldDef::new("token", FieldType::String)))
            .unwrap();
        let mut f = Document::new();
        f.insert("id".into(), Value::Text("s0".into()));
        f.insert("token".into(), Value::Text(SECRET.into()));
        engine.insert("S", f).unwrap();
    }
    let dump_dir = tempfile::tempdir().unwrap();
    let dump = dump_dir.path().join("backup.jsonl");
    cmd_dump(src.path(), &dump).unwrap();
    let (report, ok) = cmd_backup_verify(&dump).unwrap();
    assert!(ok);
    assert!(
        !report.contains(SECRET),
        "verify report must not echo record field values"
    );
}

#[test]
fn restore_line_limit_error_structured() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("oversize.jsonl");
    // One line longer than the per-line restore bound: it must be refused before
    // being buffered whole, with a message that names the limit.
    let mut data = Vec::with_capacity(MAX_RESTORE_LINE_BYTES + 32);
    data.extend_from_slice(b"{\"type\":\"record\",\"collection\":\"C\",\"fields\":{\"id\":\"");
    data.resize(MAX_RESTORE_LINE_BYTES + 16, b'x');
    std::fs::write(&dump, &data).unwrap();

    let dst = tempfile::tempdir().unwrap();
    let err = cmd_restore(dst.path(), &dump).unwrap_err();
    assert!(
        err.to_string().contains("restore limit"),
        "oversize line error should name the restore limit: {err}"
    );
}
