//! Upgrade coverage: open an AuraDB v0.1.0 data directory with the current
//! engine, validate it, and confirm indexes rebuild and data is intact.
//!
//! The fixture under `tests/fixtures/v0_1_0_data/` was written by the AuraDB
//! v0.1.0 binary (see `tests/fixtures/README.md`). The on-disk storage format is
//! unchanged from v0.1.0, so the upgrade path is: open the directory, validate
//! the manifest and catalog, and rebuild persisted indexes from storage (v0.1.0
//! did not persist indexes). This test is a real upgrade check, not a
//! current-version fixture relabelled as v0.1.0.

use std::path::{Path, PathBuf};

use auradb::core::Value;
use auradb::query::FindQuery;
use auradb::Engine;

/// Locate the committed v0.1.0 fixture relative to this crate.
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/v0_1_0_data")
}

/// Recursively copy a directory tree.
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
fn staged_fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    copy_dir(&fixture_dir(), &data);
    (tmp, data)
}

#[test]
fn opens_v0_1_0_directory_and_rebuilds_indexes() {
    let (_tmp, data) = staged_fixture();

    // v0.1.0 did not persist indexes, so opening rebuilds every collection's
    // indexes from storage rather than loading a snapshot.
    let engine = Engine::open(&data).expect("v0.2.x opens a v0.1.0 data directory");
    let report = engine.index_load_report();
    assert_eq!(
        report.loaded, 0,
        "a v0.1.0 directory has no index snapshots"
    );
    assert!(report.rebuilt >= 2, "indexes rebuilt from storage");

    // Schema catalog loads.
    let schemas: Vec<String> = engine.list_schemas().into_iter().map(|s| s.name).collect();
    assert!(schemas.contains(&"Org".to_string()));
    assert!(schemas.contains(&"User".to_string()));

    // Records load.
    let users = engine.find(&FindQuery::new("User")).unwrap();
    assert_eq!(users.len(), 10);
    let orgs = engine.find(&FindQuery::new("Org")).unwrap();
    assert_eq!(orgs.len(), 3);

    // Document, vector, and relationship data survived the upgrade.
    let u0 = users
        .iter()
        .find(|r| matches!(r.fields.get("id"), Some(Value::Text(t)) if t == "user0"))
        .expect("user0 present");
    assert!(matches!(u0.fields.get("profile"), Some(Value::Object(_))));
    assert!(matches!(u0.fields.get("embedding"), Some(Value::Vector(v)) if v.len() == 4));
    assert!(matches!(u0.fields.get("org"), Some(Value::Text(t)) if t == "org0"));

    // Consistency check passes after the upgrade.
    let verified = engine.check_consistency().unwrap();
    assert_eq!(verified, 13);
}

#[test]
fn rebuilt_indexes_serve_equality_lookups_after_upgrade() {
    use auradb::query::{CompareOp, Filter};
    let (_tmp, data) = staged_fixture();
    let engine = Engine::open(&data).unwrap();

    // The secondary index on User.name was rebuilt from storage.
    let mut q = FindQuery::new("User");
    q.filter = Some(Filter::Compare {
        field: "name".into(),
        op: CompareOp::Eq,
        value: Value::Text("User 4".into()),
    });
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].fields.get("id"), Some(Value::Text(t)) if t == "user4"));
}

#[test]
fn backup_after_upgrade_roundtrips() {
    let (_tmp, data) = staged_fixture();
    let engine = Engine::open(&data).unwrap();

    // Dump every collection at the engine level, then restore into a fresh dir.
    let dest = tempfile::tempdir().unwrap();
    let restored = Engine::open(dest.path()).unwrap();
    for schema in engine.list_schemas() {
        restored.create_schema(schema.clone()).unwrap();
        for row in engine.find(&FindQuery::new(&schema.name)).unwrap() {
            restored
                .apply_mutation(auradb::query::Mutation::Upsert {
                    collection: schema.name.clone(),
                    fields: row.fields,
                })
                .unwrap();
        }
    }
    assert_eq!(restored.find(&FindQuery::new("User")).unwrap().len(), 10);
    assert_eq!(restored.check_consistency().unwrap(), 13);
}

#[test]
fn incompatible_future_format_is_rejected_not_silently_downgraded() {
    let (_tmp, data) = staged_fixture();
    // Simulate a directory written by a future, incompatible format version.
    let manifest_path = data.join("MANIFEST");
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    let bumped = text.replace("\"format_version\": 1", "\"format_version\": 999");
    assert_ne!(text, bumped, "manifest format_version was rewritten");
    std::fs::write(&manifest_path, bumped).unwrap();

    let err = Engine::open(&data);
    assert!(
        err.is_err(),
        "an unknown future storage format must be rejected, never silently opened"
    );
}
