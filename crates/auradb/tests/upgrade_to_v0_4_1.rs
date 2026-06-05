//! Upgrade safety into AuraDB v0.4.1.
//!
//! v0.4.1 is a patch release: it changes no on-disk format. It opens genuine data
//! directories written by earlier release binaries with the current engine and
//! verifies the Raft/replication hardening does not disturb existing data —
//! non-cluster data opens unchanged, MVCC/indexes remain valid, backup works, and
//! single-node cluster mode still works after upgrade.
//!
//! For the v0.4.0 -> v0.4.1 step specifically: v0.4.0 and v0.4.1 share every
//! on-disk format (storage v2, cluster metadata v1, Raft log + hard state, the
//! snapshot manifest). v0.4.0 fixtures are produced here by exercising the cluster
//! path (which writes exactly the v0.4.0 layout, all `format_version` 1), then the
//! data is reopened to prove the v0.4.1 code reads it, that compaction metadata
//! initializes safely where none existed, and that v0.4.0-shaped snapshot
//! manifests still decode. Unknown future formats are rejected (fail closed).

use std::path::{Path, PathBuf};

use auradb::query::{CountQuery, FindQuery, Mutation};
use auradb::Engine;
use auradb_cluster::{ClusterConfig, ClusterStore};
use auradb_replication::{ClusterNode, SnapshotManifest};

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

/// The shared upgrade checklist for a pre-0.4.x fixture.
fn run_upgrade(name: &str) {
    let (_tmp, data) = staged(name);

    // 1. Non-cluster data opens unchanged.
    let engine = Engine::open(&data).expect("v0.4.1 opens the fixture");
    let schemas = engine.list_schemas();
    assert!(!schemas.is_empty(), "{name}: schemas opened");
    let records = total_records(&engine);
    assert!(records > 0, "{name}: records opened");

    // 2. Indexes valid; MVCC and planner stats valid.
    let report = engine.index_load_report();
    assert_eq!(report.loaded + report.rebuilt, schemas.len());
    engine.check_consistency().expect("indexes consistent");
    engine.analyze().expect("analyze after upgrade");

    // 3. A plain open creates no cluster identity.
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

    // 5. Single-node cluster mode works on the upgraded directory, and the log
    //    can then be compacted up to the applied prefix.
    let identity = ClusterStore::new(&data).init(None, None, "0.4.1").unwrap();
    let node = ClusterNode::bootstrap(
        engine.clone(),
        identity,
        ClusterConfig::single_node(),
        data.join("cluster"),
    )
    .expect("single-node cluster bootstraps on upgraded data");
    engine.attach_replicated_log(node.write_log());

    let collection = schemas[0].name.clone();
    let mut fields = auradb::core::Document::new();
    if let Some(pk) = schemas[0].primary_key() {
        fields.insert(pk.name.clone(), sample_value(&pk.field_type));
    }
    for f in &schemas[0].fields {
        if !f.nullable && !fields.contains_key(&f.name) {
            fields.insert(f.name.clone(), sample_value(&f.field_type));
        }
    }
    engine
        .apply_mutation(Mutation::Upsert {
            collection: collection.clone(),
            fields,
        })
        .expect("cluster-mode write succeeds");
    // Compaction metadata initializes safely (there was none before).
    let compaction = node.compact_log(false).expect("compaction succeeds");
    assert!(
        compaction.last_included_index <= compaction.commit_index,
        "{name}: compaction stays within the committed prefix"
    );
}

fn sample_value(ty: &auradb::core::FieldType) -> auradb::core::Value {
    use auradb::core::{FieldType, Value};
    match ty {
        FieldType::Int => Value::Int(987_654_321),
        FieldType::Timestamp => Value::Int(1_900_000_000_000),
        FieldType::Float => Value::Float(1.0),
        FieldType::Bool => Value::Bool(true),
        FieldType::Uuid => Value::Text("99999999-9999-4999-8999-999999999999".into()),
        _ => Value::Text("auradb-v041-upgrade-probe".into()),
    }
}

#[test]
fn v0_1_0_fixture_to_v0_4_1() {
    run_upgrade("v0_1_0_data");
}

#[test]
fn v0_2_0_fixture_to_v0_4_1() {
    run_upgrade("v0_2_0_data");
}

#[test]
fn v0_2_1_fixture_to_v0_4_1() {
    run_upgrade("v0_2_1_data");
}

#[test]
fn v0_3_0_fixture_to_v0_4_1() {
    run_upgrade("v0_3_0_data");
}

/// v0.3.0 and v0.3.1 share storage format v2; this covers the v0.3.1 path.
#[test]
fn v0_3_1_equivalent_fixture_to_v0_4_1() {
    run_upgrade("v0_3_0_data");
}

/// v0.4.0 -> v0.4.1: the cluster, Raft, and commit-base files a v0.4.0 cluster
/// writes (all `format_version` 1) reopen unchanged under v0.4.1, and compaction
/// metadata initializes safely where the v0.4.0 layout had none.
#[test]
fn v0_4_0_cluster_layout_opens_under_v0_4_1() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    // Produce the v0.4.0 on-disk cluster layout: identity, raft log, hard state,
    // and commit base. (v0.4.0 and v0.4.1 share these formats exactly.)
    {
        let engine = Engine::open(&data).unwrap();
        engine
            .create_schema(
                auradb::core::CollectionSchema::new("C")
                    .with_field(auradb::core::FieldDef {
                        name: "id".into(),
                        field_type: auradb::core::FieldType::Int,
                        primary_key: true,
                        unique: true,
                        nullable: false,
                        indexed: false,
                    })
                    .with_field(auradb::core::FieldDef::new(
                        "v",
                        auradb::core::FieldType::Int,
                    )),
            )
            .unwrap();
        let identity = ClusterStore::new(&data).init(None, None, "0.4.0").unwrap();
        let node = ClusterNode::bootstrap(
            engine.clone(),
            identity,
            ClusterConfig::single_node(),
            data.join("cluster"),
        )
        .unwrap();
        engine.attach_replicated_log(node.write_log());
        for id in 1..=3 {
            let mut f = auradb::core::Document::new();
            f.insert("id".into(), auradb::core::Value::Int(id));
            f.insert("v".into(), auradb::core::Value::Int(id));
            engine
                .apply_mutation(Mutation::Insert {
                    collection: "C".into(),
                    fields: f,
                })
                .unwrap();
        }
    }
    // No compaction metadata was written by the v0.4.0-style layout.
    assert!(!data.join("cluster").join("raft-compaction.json").exists());

    // Reopen under the current (v0.4.1) code: data, cluster metadata, raft log,
    // and commit base all open, and compaction metadata initializes safely.
    let engine = Engine::open(&data).unwrap();
    assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 3);
    let identity = ClusterStore::new(&data).load().unwrap().unwrap();
    let node = ClusterNode::bootstrap(
        engine.clone(),
        identity,
        ClusterConfig::single_node(),
        data.join("cluster"),
    )
    .expect("v0.4.0 cluster layout reopens under v0.4.1");
    engine.attach_replicated_log(node.write_log());
    assert_eq!(
        engine.find(&FindQuery::new("C")).unwrap().len(),
        3,
        "records survive the v0.4.0 -> v0.4.1 reopen"
    );
    // Compaction now works on the upgraded layout.
    let report = node.compact_log(true).unwrap();
    assert!(report.last_included_index <= report.commit_index);
}

/// A v0.4.0-shaped snapshot manifest (without the v0.4.1 identity/provenance
/// fields) still decodes under v0.4.1: the new fields are additive and optional.
#[test]
fn v0_4_0_snapshot_manifest_decodes() {
    // The minimal v0.4.0 manifest shape: an empty logical dump payload.
    let payload = serde_json::to_vec(&serde_json::json!({
        "schemas": [],
        "records": [],
    }))
    .unwrap();
    let digest = crc32_of(&payload);
    let manifest = serde_json::json!({
        "meta": {
            "format_version": 1,
            "last_included_index": 7,
            "last_included_term": 2,
            "digest": digest,
            "created_by_version": "0.4.0",
        },
        "payload": payload,
    });
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let decoded = SnapshotManifest::decode(&bytes).expect("v0.4.0 manifest decodes");
    assert_eq!(decoded.meta.last_included_index, 7);
    assert_eq!(decoded.meta.cluster_id, None);
    // Defaults fill in for the absent fields.
    assert_eq!(
        decoded.meta.storage_format_version,
        auradb_storage::FORMAT_VERSION
    );
    assert!(decoded.verified_payload().is_ok());
}

/// `payload` is serialized as a JSON array of bytes by serde, so compute the
/// digest the same way the library does.
fn crc32_of(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

#[test]
fn unknown_future_formats_are_rejected() {
    // Future cluster format.
    let (_tmp, data) = staged("v0_3_0_data");
    let store = ClusterStore::new(&data);
    store.init(None, None, "0.4.1").unwrap();
    let node_json = data.join("cluster").join("node.json");
    let text = std::fs::read_to_string(&node_json).unwrap();
    std::fs::write(
        &node_json,
        text.replace("\"format_version\": 1", "\"format_version\": 9999"),
    )
    .unwrap();
    assert!(
        ClusterStore::new(&data).load().is_err(),
        "an unknown future cluster format is rejected"
    );

    // Future snapshot format.
    let bad_snapshot = serde_json::json!({
        "meta": {
            "format_version": 9999,
            "last_included_index": 1,
            "last_included_term": 1,
            "digest": 0,
            "created_by_version": "9.9.9",
        },
        "payload": [],
    });
    let bytes = serde_json::to_vec(&bad_snapshot).unwrap();
    assert!(
        SnapshotManifest::decode(&bytes).is_err(),
        "an unknown future snapshot format is rejected"
    );
}
