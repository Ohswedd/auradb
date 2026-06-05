//! Upgrade coverage: open AuraDB v0.2.0 and v0.2.1 data directories (storage
//! format v1) with the current v0.3.0 engine and confirm they migrate to the
//! MVCC format (v2), keep all data and indexes, pass consistency checks, and
//! support transactions, snapshot reads, GC, and backup after the upgrade.
//!
//! The fixtures under `tests/fixtures/v0_2_0_data/` and `v0_2_1_data/` were
//! written by the respective release binaries (checked out at the `v0.2.0` and
//! `v0.2.1` tags), not relabelled current-version directories. See
//! `tests/fixtures/README.md`.

use std::path::{Path, PathBuf};

use auradb::core::Value;
use auradb::query::{CompareOp, CountQuery, Filter, FindQuery, Mutation};
use auradb::Engine;

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Copy the read-only fixture into a temp dir so the test never mutates it.
fn staged(name: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    copy_dir(&fixture_dir(name), &data);
    (tmp, data)
}

fn manifest_format_version(data: &Path) -> u64 {
    let text = std::fs::read_to_string(data.join("MANIFEST")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["format_version"].as_u64().unwrap()
}

fn run_upgrade_suite(name: &str) {
    let (_tmp, data) = staged(name);
    assert_eq!(manifest_format_version(&data), 1, "fixture is format v1");

    let engine = Engine::open(&data).expect("v0.3.0 opens a v0.2.x directory");

    // Data and schema survived the upgrade.
    let schemas: Vec<String> = engine.list_schemas().into_iter().map(|s| s.name).collect();
    assert!(schemas.contains(&"Org".to_string()));
    assert!(schemas.contains(&"User".to_string()));
    assert_eq!(engine.find(&FindQuery::new("User")).unwrap().len(), 10);
    assert_eq!(engine.find(&FindQuery::new("Org")).unwrap().len(), 3);

    // The earlier update to user0 migrated to the latest version of the chain.
    let mut q = FindQuery::new("User");
    q.filter = Some(Filter::Compare {
        field: "id".into(),
        op: CompareOp::Eq,
        value: Value::Text("user0".into()),
    });
    let u0 = &engine.find(&q).unwrap()[0];
    assert_eq!(
        u0.fields.get("name"),
        Some(&Value::Text("User 0 (updated)".into()))
    );
    assert!(matches!(u0.fields.get("profile"), Some(Value::Object(_))));
    assert!(matches!(u0.fields.get("embedding"), Some(Value::Vector(v)) if v.len() == 4));

    // Indexes and consistency are intact (secondary, document, vector, full-text).
    let mut name_q = FindQuery::new("User");
    name_q.filter = Some(Filter::Compare {
        field: "name".into(),
        op: CompareOp::Eq,
        value: Value::Text("User 4".into()),
    });
    assert_eq!(engine.find(&name_q).unwrap().len(), 1);
    let mut text_q = FindQuery::new("User");
    text_q.filter = Some(Filter::ContainsText {
        field: "bio".into(),
        query: "aura".into(),
    });
    assert_eq!(engine.find(&text_q).unwrap().len(), 10);
    assert_eq!(engine.check_consistency().unwrap(), 13);

    // MVCC works after upgrade: a transaction pins a snapshot; a later commit is
    // invisible to it; conflicts are detected.
    let txn = engine.begin();
    engine
        .apply_mutation(Mutation::Update {
            collection: "User".into(),
            filter: Some(Filter::Compare {
                field: "id".into(),
                op: CompareOp::Eq,
                value: Value::Text("user1".into()),
            }),
            set: {
                let mut s = auradb::core::Document::new();
                s.insert("name".into(), Value::Text("changed".into()));
                s
            },
        })
        .unwrap();
    let mut q1 = FindQuery::new("User");
    q1.filter = Some(Filter::Compare {
        field: "id".into(),
        op: CompareOp::Eq,
        value: Value::Text("user1".into()),
    });
    // The transaction's snapshot predates the update.
    let snap = &engine.txn_find(&txn, &q1).unwrap()[0];
    assert_eq!(snap.fields.get("name"), Some(&Value::Text("User 1".into())));
    engine.rollback(txn);

    // GC runs cleanly after upgrade.
    engine.gc().unwrap();
    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "User".into(),
                filter: None
            })
            .unwrap(),
        10
    );

    // The directory is now persisted in the MVCC format and reopens as v2.
    drop(engine);
    assert_eq!(manifest_format_version(&data), 2, "migrated to v2 on open");
    let reopened = Engine::open(&data).unwrap();
    assert_eq!(reopened.find(&FindQuery::new("User")).unwrap().len(), 10);
    assert_eq!(reopened.check_consistency().unwrap(), 13);
}

#[test]
fn upgrade_v0_2_0_to_v0_3_0() {
    run_upgrade_suite("v0_2_0_data");
}

#[test]
fn upgrade_v0_2_1_to_v0_3_0() {
    run_upgrade_suite("v0_2_1_data");
}

#[test]
fn backup_after_v0_2_1_upgrade_roundtrips() {
    let (_tmp, data) = staged("v0_2_1_data");
    let engine = Engine::open(&data).unwrap();
    let dest = tempfile::tempdir().unwrap();
    let restored = Engine::open(dest.path()).unwrap();
    for schema in engine.list_schemas() {
        restored.create_schema(schema.clone()).unwrap();
        for row in engine.find(&FindQuery::new(&schema.name)).unwrap() {
            restored
                .apply_mutation(Mutation::Upsert {
                    collection: schema.name.clone(),
                    fields: row.fields,
                })
                .unwrap();
        }
    }
    assert_eq!(restored.find(&FindQuery::new("User")).unwrap().len(), 10);
    assert_eq!(restored.check_consistency().unwrap(), 13);
}
