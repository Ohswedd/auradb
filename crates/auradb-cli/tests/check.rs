//! Storage corruption drills for `auradb check` / `auradb check --json`.
//!
//! Each test builds a real data directory, injects one specific kind of
//! corruption, and asserts that the structured [`CheckReport`] attributes the
//! fault to the right layer. Fatal faults (storage, catalog, raft, snapshot)
//! make `ok` false; recoverable conditions (index rebuilds, corrupt advisory
//! statistics) stay non-fatal warnings.

use std::path::Path;

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::Engine;
use auradb_cli::{check_report, cmd_check_json};
use auradb_raft::{
    Command, CommandKind, FileStorage, HardState, LogEntry, LogIndex, RaftStorage, Term,
};

/// Build a healthy single-node data directory exercising a secondary index, a
/// full-text index, persisted index snapshots, and persisted planner statistics.
fn build_db(dir: &Path) {
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
        .with_index(IndexDef {
            path: "title".into(),
            kind: IndexKind::FullText,
        });
    engine.create_schema(schema).unwrap();
    for i in 0..8 {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("doc{i}")));
        f.insert(
            "title".into(),
            Value::Text(format!("the quick brown fox {i}")),
        );
        engine.insert("Doc", f).unwrap();
    }
    engine.persist_indexes().unwrap();
    engine.analyze().unwrap();
    // Drop the engine so all files are flushed before we corrupt them.
    drop(engine);
}

/// Write a valid durable Raft log into `<dir>/cluster` (cluster-mode fixture).
fn seed_raft(dir: &Path) -> std::path::PathBuf {
    let cluster = dir.join("cluster");
    std::fs::create_dir_all(&cluster).unwrap();
    let mut s = FileStorage::open(&cluster).unwrap();
    s.append(&[LogEntry {
        term: Term(1),
        index: LogIndex(1),
        command: Command::new(CommandKind::Database, b"abc".to_vec()),
    }])
    .unwrap();
    s.save_hard_state(&HardState {
        current_term: Term(1),
        voted_for: None,
        commit_index: LogIndex(1),
    })
    .unwrap();
    drop(s);
    cluster
}

#[test]
fn check_reports_manifest_corruption() {
    let dir = tempfile::tempdir().unwrap();
    build_db(dir.path());
    std::fs::write(dir.path().join("MANIFEST"), b"not valid json {{{").unwrap();

    let report = check_report(dir.path());
    assert!(!report.ok, "corrupt manifest must fail the check");
    assert!(!report.storage.ok);
    assert!(report
        .storage
        .error
        .as_deref()
        .unwrap()
        .contains("manifest"));
    assert!(!report.errors.is_empty());
}

#[test]
fn check_rejects_unknown_future_format() {
    let dir = tempfile::tempdir().unwrap();
    build_db(dir.path());
    let manifest = dir.path().join("MANIFEST");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let bumped = text.replace("\"format_version\": 2", "\"format_version\": 999");
    assert_ne!(text, bumped, "expected to rewrite the format version");
    std::fs::write(&manifest, bumped).unwrap();

    let report = check_report(dir.path());
    assert!(!report.ok, "an unknown future format must be rejected");
    assert!(!report.storage.ok);
    assert_eq!(report.storage.format_version, Some(999));
}

#[test]
fn check_reports_catalog_corruption() {
    let dir = tempfile::tempdir().unwrap();
    build_db(dir.path());
    std::fs::write(dir.path().join("catalog.json"), b"}{ broken").unwrap();

    let report = check_report(dir.path());
    assert!(!report.ok, "corrupt catalog must fail the check");
    assert!(!report.catalog.ok);
    assert!(report.catalog.error.is_some());
}

#[test]
fn check_reports_segment_checksum_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    build_db(dir.path());
    let seg = dir.path().join("0000000001.seg");
    let mut bytes = std::fs::read(&seg).unwrap();
    assert!(bytes.len() > 20, "segment should hold committed batches");
    // Flip the last byte of the final committed frame's payload: a complete frame
    // with a bad checksum (not a torn tail, which would recover silently).
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&seg, bytes).unwrap();

    let report = check_report(dir.path());
    assert!(!report.ok, "a checksum mismatch must fail the check");
    assert!(!report.storage.ok);
    assert!(!report.storage.opened);
}

#[test]
fn check_reports_index_manifest_corruption() {
    let dir = tempfile::tempdir().unwrap();
    build_db(dir.path());
    let index_manifest = dir.path().join("indexes").join("INDEX_MANIFEST.json");
    assert!(
        index_manifest.exists(),
        "persist_indexes should write a manifest"
    );
    std::fs::write(&index_manifest, b"garbage").unwrap();

    // A corrupt index manifest is repaired (rebuilt from storage) on open, so the
    // overall check stays healthy but reports the rebuild as a warning.
    let report = check_report(dir.path());
    assert!(report.ok, "index corruption is recoverable by rebuild");
    assert_eq!(report.indexes.consistency_ok, Some(true));
    assert!(report.indexes.rebuilt.unwrap() > 0);
    assert!(report.warnings.iter().any(|w| w.contains("rebuilt")));
}

#[test]
fn check_reports_planner_stats_corruption() {
    let dir = tempfile::tempdir().unwrap();
    build_db(dir.path());
    let stats = dir.path().join("planner_stats.json");
    assert!(stats.exists(), "analyze should write planner statistics");
    std::fs::write(&stats, b"not json").unwrap();

    // Planner statistics are advisory: a corrupt file is a warning, not fatal.
    let report = check_report(dir.path());
    assert!(report.ok, "corrupt advisory stats must not be fatal");
    assert!(!report.planner_stats.ok);
    assert!(report.warnings.iter().any(|w| w.contains("statistics")));
}

#[test]
fn check_reports_raft_log_corruption() {
    let dir = tempfile::tempdir().unwrap();
    build_db(dir.path());
    let cluster = seed_raft(dir.path());
    let log = cluster.join("raft-log.bin");
    let mut bytes = std::fs::read(&log).unwrap();
    let n = bytes.len();
    bytes[n - 1] ^= 0xff; // corrupt a complete frame
    std::fs::write(&log, bytes).unwrap();

    let report = check_report(dir.path());
    assert!(!report.ok, "corrupt raft log must fail the check");
    assert!(report.raft.present);
    assert!(!report.raft.ok);
}

#[test]
fn check_reports_snapshot_manifest_corruption() {
    let dir = tempfile::tempdir().unwrap();
    build_db(dir.path());
    let cluster = dir.path().join("cluster");
    std::fs::create_dir_all(&cluster).unwrap();
    std::fs::write(cluster.join("raft-compaction.json"), b"{ broken").unwrap();

    let report = check_report(dir.path());
    assert!(!report.ok, "corrupt snapshot boundary must fail the check");
    assert!(report.snapshots.boundary_present);
    assert!(!report.snapshots.ok);
}

#[test]
fn check_json_shape_stable() {
    let dir = tempfile::tempdir().unwrap();
    build_db(dir.path());

    let (json, ok) = cmd_check_json(dir.path()).unwrap();
    assert!(ok, "a freshly built database should pass");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    for key in [
        "ok",
        "auradb_version",
        "data_dir",
        "storage",
        "catalog",
        "indexes",
        "planner_stats",
        "raft",
        "snapshots",
        "warnings",
        "errors",
    ] {
        assert!(v.get(key).is_some(), "missing top-level field `{key}`");
    }
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(
        v["storage"]["max_readable_format_version"],
        serde_json::json!(2)
    );
}

#[test]
fn check_never_prints_secrets() {
    let dir = tempfile::tempdir().unwrap();
    build_db(dir.path());
    // Drop a secret-looking file into the data dir; the check must never surface
    // its contents (it only inspects on-disk database state, never config/TLS).
    std::fs::write(
        dir.path().join("AuraDB.toml"),
        "token_hash = \"$argon2id$v=19$m=19456,t=2,p=1$c2VjcmV0$deadbeef\"\n",
    )
    .unwrap();

    let (json, _) = cmd_check_json(dir.path()).unwrap();
    let lower = json.to_lowercase();
    assert!(!lower.contains("argon2"), "report leaked a token hash");
    assert!(!json.contains("deadbeef"), "report leaked secret material");
    assert!(
        !lower.contains("token_hash"),
        "report leaked a secret field"
    );
}
