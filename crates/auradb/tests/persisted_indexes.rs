//! Persisted index tests: snapshots are created, loaded on restart, and safely
//! rebuilt when missing, corrupt, or stale, never returning wrong results.

use std::fs;
use std::path::Path;

use auradb::core::{CollectionId, CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{CompareOp, Filter, FindQuery, Mutation};
use auradb::Engine;

fn id_eq(value: &str) -> Filter {
    Filter::Compare {
        field: "id".into(),
        op: CompareOp::Eq,
        value: Value::Text(value.into()),
    }
}

fn doc_schema() -> CollectionSchema {
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
            name: "email".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: true,
            nullable: true,
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
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
}

fn doc(id: &str, email: &str, status: &str, vec: Vec<f32>) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(id.into()));
    f.insert("email".into(), Value::Text(email.into()));
    f.insert("status".into(), Value::Text(status.into()));
    f.insert("embedding".into(), Value::Vector(vec));
    f
}

fn seed(engine: &Engine) {
    engine.create_schema(doc_schema()).unwrap();
    engine
        .insert(
            "Doc",
            doc("d1", "a@x.com", "published", vec![1.0, 0.0, 0.0]),
        )
        .unwrap();
    engine
        .insert("Doc", doc("d2", "b@x.com", "draft", vec![0.0, 1.0, 0.0]))
        .unwrap();
    engine
        .insert(
            "Doc",
            doc("d3", "c@x.com", "published", vec![0.9, 0.1, 0.0]),
        )
        .unwrap();
}

fn status_eq(value: &str) -> Filter {
    Filter::Compare {
        field: "status".into(),
        op: CompareOp::Eq,
        value: Value::Text(value.into()),
    }
}

fn index_files(dir: &Path) -> Vec<String> {
    let idir = dir.join("indexes");
    fs::read_dir(&idir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".idx"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn index_file_created_after_checkpoint_and_loaded_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seed(&engine);
        engine.checkpoint().unwrap();
        assert!(
            !index_files(dir.path()).is_empty(),
            "a .idx file should exist"
        );
        assert!(dir.path().join("indexes/INDEX_MANIFEST.json").exists());
    }
    // Restart: the index is loaded from the snapshot, not rebuilt.
    let engine = Engine::open(dir.path()).unwrap();
    let report = engine.index_load_report();
    assert_eq!(report.rebuilt, 0, "indexes should load from disk");
    assert_eq!(report.loaded, 1);
    // Secondary filter still works after restart.
    let mut q = FindQuery::new("Doc");
    q.filter = Some(status_eq("published"));
    assert_eq!(engine.find(&q).unwrap().len(), 2);
}

#[test]
fn missing_index_file_is_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seed(&engine);
        engine.checkpoint().unwrap();
    }
    // Delete the snapshot file (manifest still references it).
    for f in index_files(dir.path()) {
        fs::remove_file(dir.path().join("indexes").join(f)).unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(engine.index_load_report().rebuilt, 1);
    let mut q = FindQuery::new("Doc");
    q.filter = Some(status_eq("draft"));
    assert_eq!(engine.find(&q).unwrap().len(), 1);
}

#[test]
fn corrupt_index_file_is_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seed(&engine);
        engine.checkpoint().unwrap();
    }
    for f in index_files(dir.path()) {
        let p = dir.path().join("indexes").join(f);
        let mut bytes = fs::read(&p).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xff;
        fs::write(&p, &bytes).unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(
        engine.index_load_report().rebuilt,
        1,
        "corrupt snapshot rebuilds"
    );
    // Results remain correct.
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 3);
}

#[test]
fn stale_index_is_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seed(&engine);
        engine.checkpoint().unwrap();
        // Mutate after the checkpoint without a fresh checkpoint: the snapshot
        // is now stale relative to storage.
        engine
            .insert(
                "Doc",
                doc("d4", "d@x.com", "published", vec![0.5, 0.5, 0.0]),
            )
            .unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(
        engine.index_load_report().rebuilt,
        1,
        "stale snapshot must be rebuilt"
    );
    let mut q = FindQuery::new("Doc");
    q.filter = Some(status_eq("published"));
    assert_eq!(engine.find(&q).unwrap().len(), 3);
}

#[test]
fn unique_constraint_enforced_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seed(&engine);
        engine.checkpoint().unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(engine.index_load_report().loaded, 1);
    // Duplicate unique email must be rejected using the loaded index.
    let dup = engine.apply_mutation(Mutation::Insert {
        collection: "Doc".into(),
        fields: doc("d9", "a@x.com", "draft", vec![0.0, 0.0, 1.0]),
    });
    assert!(
        dup.is_err(),
        "unique email constraint must hold after restart"
    );
}

#[test]
fn delete_and_update_reflected_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seed(&engine);
        engine
            .apply_mutation(Mutation::Delete {
                collection: "Doc".into(),
                filter: Some(id_eq("d3")),
            })
            .unwrap();
        engine
            .apply_mutation(Mutation::Update {
                collection: "Doc".into(),
                filter: Some(id_eq("d2")),
                set: {
                    let mut s = Document::new();
                    s.insert("status".into(), Value::Text("published".into()));
                    s
                },
            })
            .unwrap();
        engine.checkpoint().unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(engine.index_load_report().loaded, 1);
    let mut q = FindQuery::new("Doc");
    q.filter = Some(status_eq("published"));
    // d1 and d2 (updated); d3 deleted.
    assert_eq!(engine.find(&q).unwrap().len(), 2);
}

#[test]
fn compaction_preserves_persisted_indexes() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seed(&engine);
        engine.compact().unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(
        engine.index_load_report().rebuilt,
        0,
        "compaction refreshes snapshots"
    );
    let _ = CollectionId::new("Doc");
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 3);
}

#[test]
fn index_rebuild_command_path() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    let report = engine.rebuild_indexes().unwrap();
    assert_eq!(report.rebuilt, 1);
    // After rebuild + persist, a fresh open loads from disk.
    drop(engine);
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(engine.index_load_report().loaded, 1);
}
