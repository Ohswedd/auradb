//! Upgrade drills into AuraDB v0.8.0.
//!
//! v0.8.0 changes no on-disk format. It opens genuine data directories written by
//! earlier release binaries with the current engine and runs the full v0.8.0
//! upgrade checklist over each: the data opens, the structured `check` passes,
//! planner statistics analyze, indexes load or rebuild and verify, a post-upgrade
//! backup round-trips through `dump` -> `backup verify` -> `restore`, and a query
//! smoke succeeds. A manifest carrying an unknown future format is rejected.
//!
//! ## Fixture coverage and representative formats
//!
//! The committed fixtures are genuine release-binary outputs covering both
//! on-disk storage families:
//!
//! * **storage format v1** — `v0_1_0_data`, `v0_2_0_data`, `v0_2_1_data`
//!   (representative of v0.1.0 through v0.2.1).
//! * **storage format v2 (MVCC)** — `v0_3_0_data` (representative of v0.3.0
//!   through v0.7.1, which all share storage `format_version` 2; v0.4.x added
//!   cluster/Raft metadata files but did not change the storage format).
//!
//! Because v0.3.x–v0.7.x share storage format v2, the v0.3.0 fixture is the
//! genuine representative for every v0.3.0 -> v0.8.0 ... v0.7.1 -> v0.8.0 step at
//! the storage layer; cluster-metadata upgrade specifics are covered separately
//! by `crates/auradb/tests/upgrade_to_v0_4_1.rs`. See `tests/fixtures/README.md`.

use std::path::{Path, PathBuf};

use auradb::query::{CountQuery, FindQuery};
use auradb::Engine;
use auradb_cli::{check_report, cmd_backup_verify, cmd_dump, cmd_restore};

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

fn staged(name: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    copy_dir(&fixture_dir(name), &data);
    (tmp, data)
}

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

/// The shared v0.8.0 upgrade checklist for one genuine fixture.
fn run_upgrade(name: &str) {
    let (_tmp, data) = staged(name);

    // 1. Open the old data.
    let engine = Engine::open(&data).unwrap_or_else(|e| panic!("{name}: open failed: {e}"));
    let schemas = engine.list_schemas();
    assert!(!schemas.is_empty(), "{name}: schemas opened");
    let records = total_records(&engine);
    assert!(records > 0, "{name}: records opened");

    // 2. Run `check` (structured) — must pass.
    let report = check_report(&data);
    assert!(report.ok, "{name}: check failed: {:?}", report.errors);

    // 3. Run `stats analyze`.
    engine
        .analyze()
        .unwrap_or_else(|e| panic!("{name}: analyze: {e}"));

    // 4. Run an index check (load report + consistency).
    let load = engine.index_load_report();
    assert_eq!(
        load.loaded + load.rebuilt,
        schemas.len(),
        "{name}: index load"
    );
    engine
        .check_consistency()
        .unwrap_or_else(|e| panic!("{name}: index consistency: {e}"));

    // 5. Backup after upgrade, then 6. verify and restore it.
    let dump_dir = tempfile::tempdir().unwrap();
    let dump = dump_dir.path().join("backup.jsonl");
    cmd_dump(&data, &dump).unwrap_or_else(|e| panic!("{name}: dump: {e}"));
    let (vr, ok) = cmd_backup_verify(&dump).unwrap();
    assert!(ok, "{name}: backup verify failed: {vr}");
    let restored_dir = tempfile::tempdir().unwrap();
    cmd_restore(restored_dir.path(), &dump).unwrap_or_else(|e| panic!("{name}: restore: {e}"));

    // 7. Query smoke against the restored copy.
    let restored = Engine::open(restored_dir.path()).unwrap();
    assert_eq!(total_records(&restored), records, "{name}: round-trips");
    let restored_report = check_report(restored_dir.path());
    assert!(restored_report.ok, "{name}: restored check passes");
    let first = restored.list_schemas()[0].name.clone();
    let _ = restored.find(&FindQuery::new(&first)).unwrap();
}

#[test]
fn upgrade_v0_1_0_to_v0_8_0() {
    run_upgrade("v0_1_0_data");
}

#[test]
fn upgrade_v0_2_0_to_v0_8_0() {
    run_upgrade("v0_2_0_data");
}

#[test]
fn upgrade_v0_2_1_to_v0_8_0() {
    run_upgrade("v0_2_1_data");
}

#[test]
fn upgrade_v0_3_0_to_v0_8_0() {
    run_upgrade("v0_3_0_data");
}

#[test]
fn upgrade_rejects_unknown_future_format() {
    let (_tmp, data) = staged("v0_3_0_data");
    let manifest = data.join("MANIFEST");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let bumped = text.replace("\"format_version\": 2", "\"format_version\": 9999");
    assert_ne!(
        text, bumped,
        "expected to rewrite the manifest format version"
    );
    std::fs::write(&manifest, bumped).unwrap();

    // Fail closed: the engine refuses the unknown format, and `check` reports it.
    assert!(
        Engine::open(&data).is_err(),
        "future format must be rejected"
    );
    let report = check_report(&data);
    assert!(!report.ok, "check must flag the future format");
    assert_eq!(report.storage.format_version, Some(9999));
}
