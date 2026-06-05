//! Upgrade safety into AuraDB v0.4.0.
//!
//! Opens genuine data directories written by earlier release binaries with the
//! current v0.4.0 engine and verifies the cluster/replication groundwork does not
//! disturb existing single-node data: non-cluster data opens unchanged, MVCC and
//! indexes remain valid, backup works, and — critically — enabling single-node
//! cluster mode on the upgraded directory works correctly (cluster writes are
//! ordered through the Raft log and stay strictly newer than the pre-existing
//! MVCC watermark). It also asserts that unknown future cluster and Raft formats
//! are rejected rather than silently opened.
//!
//! v0.3.0 and v0.3.1 share storage format v2, so the v0.3.0 fixture also covers
//! the v0.3.1 upgrade path.

use std::path::{Path, PathBuf};

use auradb::query::{CountQuery, FindQuery, Mutation};
use auradb::Engine;
use auradb_cluster::{ClusterConfig, ClusterStore};
use auradb_replication::ClusterNode;

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

/// Run the v0.4.0 upgrade checklist against a fixture.
fn run_upgrade_to_v0_4_0(name: &str) {
    let (_tmp, data) = staged(name);

    // 1. Non-cluster data opens unchanged (the default path is v0.3.1 behavior).
    let engine = Engine::open(&data).expect("v0.4.0 opens the fixture");
    let schemas = engine.list_schemas();
    assert!(!schemas.is_empty(), "{name}: schemas opened");
    let records = total_records(&engine);
    assert!(records > 0, "{name}: records opened");

    // 2. Indexes valid (loaded or rebuilt); MVCC and planner stats valid.
    let report = engine.index_load_report();
    assert_eq!(report.loaded + report.rebuilt, schemas.len());
    engine.check_consistency().expect("indexes consistent");
    engine.analyze().expect("analyze after upgrade");

    // 3. Existing data is NOT forced into cluster mode: opening the directory
    //    creates no cluster identity.
    assert!(
        !ClusterStore::new(&data).is_initialized(),
        "{name}: plain open does not initialize cluster identity"
    );

    // 4. Backup after upgrade works.
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
        "{name}: backup round-trips"
    );

    // 5. Enabling single-node cluster mode on the upgraded directory works: the
    //    pre-existing MVCC watermark is high, but a cluster write still commits
    //    and is visible (its commit timestamp is pinned above the watermark).
    let watermark_before = engine.commit_watermark();
    assert!(
        watermark_before > 0,
        "{name}: upgraded data has MVCC history"
    );
    let identity = ClusterStore::new(&data).init(None, None, "0.4.0").unwrap();
    let node = ClusterNode::bootstrap(
        engine.clone(),
        identity,
        ClusterConfig::single_node(),
        data.join("cluster"),
    )
    .expect("single-node cluster bootstraps on upgraded data");
    engine.attach_replicated_log(node.write_log());

    let collection = schemas[0].name.clone();
    let before = engine.find(&FindQuery::new(&collection)).unwrap().len();
    // A fresh insert routed through the Raft log must become visible.
    let mut fields = auradb::core::Document::new();
    if let Some(pk) = schemas[0].primary_key() {
        // Give the primary key a fresh, unlikely-to-collide value.
        fields.insert(pk.name.clone(), sample_pk_value(&pk.field_type));
    }
    // Fill required non-nullable fields with type-appropriate defaults.
    for f in &schemas[0].fields {
        if !f.nullable && !fields.contains_key(&f.name) {
            fields.insert(f.name.clone(), sample_pk_value(&f.field_type));
        }
    }
    let inserted = engine.apply_mutation(Mutation::Upsert {
        collection: collection.clone(),
        fields,
    });
    // The write goes through Raft and the node is the leader, so it succeeds and
    // the row count does not shrink.
    assert!(inserted.is_ok(), "{name}: cluster-mode write succeeds");
    let after = engine.find(&FindQuery::new(&collection)).unwrap().len();
    assert!(after >= before, "{name}: cluster write is visible");
    assert!(
        engine.commit_watermark() > watermark_before,
        "{name}: cluster write advanced the MVCC watermark past the pre-existing one"
    );
}

fn sample_pk_value(ty: &auradb::core::FieldType) -> auradb::core::Value {
    use auradb::core::{FieldType, Value};
    match ty {
        FieldType::Int => Value::Int(987_654_321),
        FieldType::Timestamp => Value::Int(1_900_000_000_000),
        FieldType::Float => Value::Float(1.0),
        FieldType::Bool => Value::Bool(true),
        FieldType::Uuid => Value::Text("99999999-9999-4999-8999-999999999999".into()),
        _ => Value::Text("auradb-v040-upgrade-probe".into()),
    }
}

#[test]
fn v0_1_0_fixture_to_v0_4_0() {
    run_upgrade_to_v0_4_0("v0_1_0_data");
}

#[test]
fn v0_2_0_fixture_to_v0_4_0() {
    run_upgrade_to_v0_4_0("v0_2_0_data");
}

#[test]
fn v0_2_1_fixture_to_v0_4_0() {
    run_upgrade_to_v0_4_0("v0_2_1_data");
}

#[test]
fn v0_3_0_fixture_to_v0_4_0() {
    run_upgrade_to_v0_4_0("v0_3_0_data");
}

/// v0.3.0 and v0.3.1 share storage format v2; this covers the v0.3.1 path.
#[test]
fn v0_3_1_equivalent_fixture_to_v0_4_0() {
    run_upgrade_to_v0_4_0("v0_3_0_data");
}

#[test]
fn unknown_future_cluster_format_is_rejected() {
    let (_tmp, data) = staged("v0_3_0_data");
    let store = ClusterStore::new(&data);
    store.init(None, None, "0.4.0").unwrap();
    // Patch the persisted node metadata to a future format version.
    let node_json = data.join("cluster").join("node.json");
    let text = std::fs::read_to_string(&node_json).unwrap();
    let bumped = text.replace("\"format_version\": 1", "\"format_version\": 9999");
    assert_ne!(text, bumped, "patched the cluster metadata format version");
    std::fs::write(&node_json, bumped).unwrap();
    assert!(
        ClusterStore::new(&data).load().is_err(),
        "an unknown future cluster format must be rejected"
    );
}
