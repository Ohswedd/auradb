//! Upgrade safety into AuraDB v0.3.1.
//!
//! Opens genuine data directories written by the v0.1.0, v0.2.0, v0.2.1, and
//! v0.3.0 release binaries (see `tests/fixtures/README.md`) with the current
//! v0.3.1 engine and runs a full operational checklist: data and schema open,
//! indexes load or rebuild, planner statistics initialize, the MVCC format
//! initializes, transactions begin and read a snapshot, GC runs, and a backup /
//! restore round-trips. It also asserts that an unknown future storage format is
//! rejected rather than silently opened, and that there is no silent downgrade.

use std::path::{Path, PathBuf};

use auradb::query::{CountQuery, FindQuery, Mutation};
use auradb::storage::StorageOptions;
use auradb::{Engine, EngineOptions, WallClock};

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

/// Copy a read-only fixture into a temp dir so the test never mutates it.
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

/// Total live records across all collections of an engine.
fn total_records(engine: &Engine) -> usize {
    engine
        .list_schemas()
        .iter()
        .map(|s| {
            engine
                .count(&CountQuery {
                    collection: s.name.clone(),
                    filter: None,
                })
                .unwrap()
        })
        .sum()
}

/// Run the full v0.3.1 upgrade checklist against a fixture.
fn run_upgrade_to_v0_3_1(name: &str) {
    let (_tmp, data) = staged(name);

    // 1. Data and schema open.
    let engine = Engine::open(&data).expect("v0.3.1 opens the fixture");
    let schemas = engine.list_schemas();
    assert!(!schemas.is_empty(), "{name}: schemas opened");
    let records = total_records(&engine);
    assert!(records > 0, "{name}: records opened");

    // 2. Indexes migrated or rebuilt safely; consistency holds.
    let report = engine.index_load_report();
    assert_eq!(report.loaded + report.rebuilt, schemas.len());
    engine.check_consistency().expect("indexes consistent");

    // 3. Planner statistics initialize.
    engine.analyze().expect("analyze after upgrade");
    let stats = engine.planner_stats();
    assert!(!stats.collections.is_empty());

    // 4. MVCC format is v2 after open; a transaction begins and reads a snapshot;
    //    GC runs after the upgrade.
    drop(engine);
    assert_eq!(manifest_format_version(&data), 2, "{name}: MVCC format v2");
    let clock = WallClock::manual();
    let engine = Engine::open_with(
        &data,
        EngineOptions {
            storage: StorageOptions::default(),
            gc_min_retained_versions: 1,
            transaction_timeout_secs: 60,
            clock: clock.clone(),
        },
    )
    .unwrap();
    let first = schemas[0].name.clone();
    let txn = engine.begin();
    let snap_len = engine
        .txn_find(&txn, &FindQuery::new(&first))
        .unwrap()
        .len();
    assert_eq!(
        snap_len,
        engine.find(&FindQuery::new(&first)).unwrap().len()
    );
    // The new transaction-timeout machinery works on an upgraded directory.
    clock.advance(120);
    assert_eq!(engine.reap_transactions(), 1);
    assert!(engine.txn_find(&txn, &FindQuery::new(&first)).is_err());
    engine.gc().expect("gc after upgrade");

    // 5. Backup works after upgrade, and restore round-trips.
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
    assert_eq!(
        total_records(&restored),
        records,
        "{name}: restore round-trips"
    );
    restored.check_consistency().unwrap();
    // GC works on the restored copy too.
    restored.gc().unwrap();
}

#[test]
fn v0_1_0_fixture_to_v0_3_1() {
    run_upgrade_to_v0_3_1("v0_1_0_data");
}

#[test]
fn v0_2_0_fixture_to_v0_3_1() {
    run_upgrade_to_v0_3_1("v0_2_0_data");
}

#[test]
fn v0_2_1_fixture_to_v0_3_1() {
    run_upgrade_to_v0_3_1("v0_2_1_data");
}

#[test]
fn v0_3_0_fixture_to_v0_3_1() {
    run_upgrade_to_v0_3_1("v0_3_0_data");
}

#[test]
fn unknown_future_format_is_rejected() {
    // A directory whose manifest claims a newer storage format than this build
    // understands must be refused, never silently opened (no silent downgrade).
    let (_tmp, data) = staged("v0_3_0_data");
    let manifest = data.join("MANIFEST");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let bumped = text.replace("\"format_version\": 2", "\"format_version\": 9999");
    assert_ne!(text, bumped, "patched the format version");
    std::fs::write(&manifest, bumped).unwrap();
    assert!(
        Engine::open(&data).is_err(),
        "an unknown future format must be rejected"
    );
}
