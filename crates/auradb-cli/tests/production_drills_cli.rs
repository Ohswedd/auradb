//! Single-node production drill coverage (release gate).
//!
//! These tests back the offline drills exercised by
//! `scripts/smoke_single_node_production_drills.sh`. They focus on the pieces
//! that script asserts but that are not already covered by `backup_drills.rs`,
//! `backup_restore_edge_cases.rs`, `large_dataset.rs`, or `check.rs`:
//!
//!   * restore-to-fresh and rollback preserve record counts,
//!   * `doctor --json` / `check --json` report a healthy state after a restore,
//!   * I/O faults (permission-denied, missing, corrupt) surface as structured
//!     errors and never panic, and
//!   * the machine-readable reports never echo record contents.
//!
//! This is a SINGLE-NODE production drill: nothing here exercises multi-node
//! replication/HA, and no test asserts approximate (ANN) recall — exact vector
//! search remains the correctness baseline.

use std::path::Path;

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::Engine;
use auradb_cli::{
    build_config, check_report, cmd_backup_verify, cmd_check_json, cmd_doctor_json, cmd_dump,
    cmd_restore, cmd_snapshot_create, cmd_snapshot_restore,
};

const DIM: usize = 4;
const SECRET_BODY: &str = "quick brown fox classified payload";

/// Build a small but realistic database: a scalar primary key, an indexed
/// string, a full-text body, and an exact vector field. Returns the live record
/// count seeded.
fn build_db(dir: &Path, n: usize) -> usize {
    let engine = Engine::open(dir).unwrap();
    let schema = CollectionSchema::new("Doc")
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
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: DIM }))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        });
    engine.create_schema(schema).unwrap();
    for i in 0..n {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("doc{i}")));
        f.insert("title".into(), Value::Text(format!("Title {i}")));
        f.insert("body".into(), Value::Text(format!("{SECRET_BODY} {i}")));
        let v: Vec<f32> = (0..DIM).map(|j| (i + j) as f32).collect();
        f.insert("embedding".into(), Value::Vector(v));
        engine.insert("Doc", f).unwrap();
    }
    engine.analyze().unwrap();
    drop(engine);
    n
}

/// The live record count a structured check reports for `dir`.
fn record_count(dir: &Path) -> usize {
    check_report(dir).storage.records.unwrap_or(usize::MAX)
}

#[test]
fn restore_to_fresh_data_dir_preserves_counts() {
    let src = tempfile::tempdir().unwrap();
    let baseline = build_db(src.path(), 50);

    let snap_dir = tempfile::tempdir().unwrap();
    let snap = snap_dir.path().join("known-good.snap");
    cmd_snapshot_create(src.path(), &snap).unwrap();

    let fresh = tempfile::tempdir().unwrap();
    // A fresh (empty) target needs no --force.
    cmd_snapshot_restore(&snap, fresh.path(), false).unwrap();

    assert_eq!(
        record_count(fresh.path()),
        baseline,
        "restore into a fresh data dir must preserve the live record count"
    );
}

#[test]
fn rollback_rehearsal_returns_to_known_good_count() {
    // Known-good state.
    let good = tempfile::tempdir().unwrap();
    let good_count = build_db(good.path(), 40);
    let snap_dir = tempfile::tempdir().unwrap();
    let snap = snap_dir.path().join("known-good.snap");
    cmd_snapshot_create(good.path(), &snap).unwrap();

    // A divergent "current" state to be rolled back.
    let current = tempfile::tempdir().unwrap();
    let diverged = build_db(current.path(), 7);
    assert_ne!(diverged, good_count);

    // Force-restore the known-good snapshot over the divergent dir.
    cmd_snapshot_restore(&snap, current.path(), true).unwrap();
    assert_eq!(
        record_count(current.path()),
        good_count,
        "rollback must return the data dir to the known-good record count"
    );
}

#[test]
fn doctor_json_after_restore_is_healthy() {
    let src = tempfile::tempdir().unwrap();
    let baseline = build_db(src.path(), 30);
    let snap_dir = tempfile::tempdir().unwrap();
    let snap = snap_dir.path().join("s.snap");
    cmd_snapshot_create(src.path(), &snap).unwrap();
    let fresh = tempfile::tempdir().unwrap();
    cmd_snapshot_restore(&snap, fresh.path(), false).unwrap();

    let config = build_config(None, Some(fresh.path().to_path_buf()), None, None, false).unwrap();
    let json = cmd_doctor_json(fresh.path(), &config).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["index_consistency_ok"], serde_json::json!(true));
    assert_eq!(v["storage_open"], serde_json::json!(true));
    assert_eq!(v["records"], serde_json::json!(baseline));
}

#[test]
fn check_json_after_restore_is_healthy() {
    let src = tempfile::tempdir().unwrap();
    let baseline = build_db(src.path(), 30);
    let snap_dir = tempfile::tempdir().unwrap();
    let snap = snap_dir.path().join("s.snap");
    cmd_snapshot_create(src.path(), &snap).unwrap();
    let fresh = tempfile::tempdir().unwrap();
    cmd_snapshot_restore(&snap, fresh.path(), false).unwrap();

    let (json, ok) = cmd_check_json(fresh.path()).unwrap();
    assert!(ok, "check must report ok after a clean restore: {json}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["storage"]["records"], serde_json::json!(baseline));
    assert_eq!(v["errors"], serde_json::json!([]));
}

#[test]
fn backup_verify_reports_valid_backup() {
    let src = tempfile::tempdir().unwrap();
    build_db(src.path(), 20);
    let dump_dir = tempfile::tempdir().unwrap();
    let dump = dump_dir.path().join("backup.jsonl");
    cmd_dump(src.path(), &dump).unwrap();

    let (report, ok) = cmd_backup_verify(&dump).unwrap();
    assert!(ok, "a faithful dump must verify: {report}");
    let v: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["records"], serde_json::json!(20));
    assert_eq!(v["collections"]["Doc"], serde_json::json!(20));
}

#[test]
fn restore_rejects_invalid_backup() {
    // A corrupt snapshot file must be rejected, not silently restored.
    let bad_dir = tempfile::tempdir().unwrap();
    let bad = bad_dir.path().join("corrupt.snap");
    std::fs::write(&bad, b"this is not a valid snapshot file").unwrap();
    let target = tempfile::tempdir().unwrap();
    let err = cmd_snapshot_restore(&bad, target.path(), true).unwrap_err();
    let msg = err.to_string();
    assert!(!msg.contains("panic"), "must be a clean error, not a panic");

    // A corrupt logical backup must be rejected by restore too.
    let corrupt = bad_dir.path().join("corrupt.jsonl");
    std::fs::write(&corrupt, b"{\"not\": valid json\n").unwrap();
    let dst = tempfile::tempdir().unwrap();
    assert!(cmd_restore(dst.path(), &corrupt).is_err());
}

#[test]
fn permission_denied_backup_path_returns_structured_error() {
    use std::os::unix::fs::PermissionsExt;
    let src = tempfile::tempdir().unwrap();
    build_db(src.path(), 5);

    let ro_parent = tempfile::tempdir().unwrap();
    let ro = ro_parent.path().join("readonly");
    std::fs::create_dir(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o000)).unwrap();

    let target = ro.join("backup.jsonl");
    let result = cmd_dump(src.path(), &target);

    // Restore writability so the tempdir can be cleaned up regardless of outcome.
    let _ = std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755));

    match result {
        Ok(_) => {
            // Running as root (or on a filesystem ignoring mode bits) bypasses the
            // permission check; the drill cannot be induced here, so don't assert.
            eprintln!("permission-denied path was writable; skipping (running as root?)");
        }
        Err(e) => {
            let msg = e.to_string();
            assert!(!msg.contains("panic"), "must be a clean error, not a panic");
            assert!(
                msg.contains("creating dump file") || msg.contains("backup.jsonl"),
                "error should name the failed dump path, got: {msg}"
            );
        }
    }
}

#[test]
fn io_error_does_not_panic() {
    let missing = Path::new("/nonexistent/aura-drill/does-not-exist");

    // Each of these references a path that cannot be opened. None may panic;
    // all must return a structured error.
    let dst = tempfile::tempdir().unwrap();
    assert!(cmd_restore(dst.path(), &missing.join("dump.jsonl")).is_err());
    assert!(cmd_backup_verify(&missing.join("backup.jsonl")).is_err());
    assert!(cmd_snapshot_restore(&missing.join("snap"), dst.path(), true).is_err());
}

#[test]
fn drill_reports_redact_record_contents() {
    // The machine-readable reports an operator captures as drill evidence must
    // never echo record field values (which may hold sensitive data).
    let src = tempfile::tempdir().unwrap();
    build_db(src.path(), 10);

    // check --json
    let (check_json, ok) = cmd_check_json(src.path()).unwrap();
    assert!(ok);
    assert!(
        !check_json.contains(SECRET_BODY),
        "check report must not echo record contents"
    );

    // doctor --json
    let config = build_config(None, Some(src.path().to_path_buf()), None, None, false).unwrap();
    let doctor_json = cmd_doctor_json(src.path(), &config).unwrap();
    assert!(
        !doctor_json.contains(SECRET_BODY),
        "doctor report must not echo record contents"
    );

    // backup verify --json (reports counts only, not values)
    let dump_dir = tempfile::tempdir().unwrap();
    let dump = dump_dir.path().join("b.jsonl");
    cmd_dump(src.path(), &dump).unwrap();
    let (verify_json, _) = cmd_backup_verify(&dump).unwrap();
    assert!(
        !verify_json.contains(SECRET_BODY),
        "backup verify report must not echo record contents"
    );
}
