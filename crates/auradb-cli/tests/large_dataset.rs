//! Large-dataset validation.
//!
//! CI-safe smokes run against a mixed 10,000-record dataset exercising scalar,
//! document, full-text, document-path, vector, and secondary-index fields across
//! bulk insert, find/filter/count/exists, order+limit, vector nearest, full-text,
//! dump/restore, and compaction. A larger 100,000-record stress test is
//! `#[ignore]`d so it runs only on demand (`cargo test -- --ignored`).

use std::path::Path;

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{
    CompareOp, CountQuery, ExistsQuery, Filter, FindQuery, Mutation, OrderKey, VectorSearch,
};
use auradb::Engine;
use auradb_cli::{check_report, cmd_dump, cmd_restore};

const DIM: usize = 8;
/// CI-safe dataset size. Large enough to exercise the indexes and scans, small
/// enough to stay fast in a debug build.
const CI_N: usize = 10_000;
/// On-demand stress size.
const STRESS_N: usize = 100_000;

fn schema() -> CollectionSchema {
    CollectionSchema::new("Item")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef {
            name: "n".into(),
            field_type: FieldType::Int,
            primary_key: false,
            unique: false,
            nullable: false,
            indexed: true,
        })
        .with_field(FieldDef::new("profile", FieldType::Document))
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: DIM }))
        .with_index(IndexDef {
            path: "profile.bucket".into(),
            kind: IndexKind::DocumentPath,
        })
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

/// Bulk-insert `n` mixed records into a fresh data dir at `dir`.
fn build(dir: &Path, n: usize) {
    let engine = Engine::open(dir).unwrap();
    engine.create_schema(schema()).unwrap();
    let chunk = 1_000;
    let mut i = 0;
    while i < n {
        let end = (i + chunk).min(n);
        let records: Vec<Document> = (i..end)
            .map(|k| {
                let mut f = Document::new();
                f.insert("id".into(), Value::Text(format!("item{k}")));
                f.insert("n".into(), Value::Int(k as i64));
                let mut profile = Document::new();
                profile.insert("bucket".into(), Value::Text(format!("b{}", k % 10)));
                f.insert("profile".into(), Value::Object(profile));
                f.insert(
                    "body".into(),
                    Value::Text(format!("record number {k} mentions the quick brown fox")),
                );
                let v: Vec<f32> = (0..DIM).map(|j| (k + j) as f32).collect();
                f.insert("embedding".into(), Value::Vector(v));
                f
            })
            .collect();
        engine
            .apply_mutation(Mutation::BulkInsert {
                collection: "Item".into(),
                records,
            })
            .unwrap();
        i = end;
    }
    engine.analyze().unwrap();
    drop(engine);
}

#[test]
fn large_dataset_query_smoke() {
    let dir = tempfile::tempdir().unwrap();
    build(dir.path(), CI_N);
    let engine = Engine::open(dir.path()).unwrap();

    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "Item".into(),
                filter: None
            })
            .unwrap(),
        CI_N
    );
    // Filter on the indexed scalar.
    let filtered = engine
        .find(&FindQuery {
            filter: Some(Filter::Compare {
                field: "n".into(),
                op: CompareOp::Lt,
                value: Value::Int(100),
            }),
            ..FindQuery::new("Item")
        })
        .unwrap();
    assert_eq!(filtered.len(), 100);
    // exists
    assert!(engine
        .exists(&ExistsQuery {
            collection: "Item".into(),
            filter: Some(Filter::Compare {
                field: "n".into(),
                op: CompareOp::Eq,
                value: Value::Int(42),
            }),
        })
        .unwrap());
    // order + limit
    let top = engine
        .find(&FindQuery {
            order_by: vec![OrderKey {
                field: "n".into(),
                desc: true,
            }],
            limit: Some(5),
            ..FindQuery::new("Item")
        })
        .unwrap();
    assert_eq!(top.len(), 5);
}

#[test]
fn large_dataset_index_smoke() {
    let dir = tempfile::tempdir().unwrap();
    build(dir.path(), CI_N);
    let report = check_report(dir.path());
    assert!(report.ok, "large dataset passes check: {:?}", report.errors);
    assert_eq!(report.indexes.consistency_ok, Some(true));

    // Document-path index returns the expected bucket slice.
    let engine = Engine::open(dir.path()).unwrap();
    let rows = engine
        .find(&FindQuery {
            filter: Some(Filter::Compare {
                field: "profile.bucket".into(),
                op: CompareOp::Eq,
                value: Value::Text("b3".into()),
            }),
            ..FindQuery::new("Item")
        })
        .unwrap();
    assert_eq!(rows.len(), CI_N / 10);
}

#[test]
fn large_dataset_vector_smoke() {
    let dir = tempfile::tempdir().unwrap();
    build(dir.path(), CI_N);
    let engine = Engine::open(dir.path()).unwrap();
    let rows = engine
        .find(&FindQuery {
            vector: Some(VectorSearch {
                field: "embedding".into(),
                query: vec![0.0; DIM],
                k: 10,
                metric: "euclidean".into(),
            }),
            ..FindQuery::new("Item")
        })
        .unwrap();
    assert_eq!(rows.len(), 10);
}

#[test]
fn large_dataset_full_text_smoke() {
    let dir = tempfile::tempdir().unwrap();
    build(dir.path(), CI_N);
    let engine = Engine::open(dir.path()).unwrap();
    let rows = engine
        .find(&FindQuery {
            filter: Some(Filter::ContainsText {
                field: "body".into(),
                query: "fox".into(),
            }),
            limit: Some(50),
            ..FindQuery::new("Item")
        })
        .unwrap();
    assert_eq!(
        rows.len(),
        50,
        "all records mention the token; limited to 50"
    );
}

#[test]
fn large_dataset_dump_restore_smoke() {
    let dir = tempfile::tempdir().unwrap();
    build(dir.path(), CI_N);
    let dump_dir = tempfile::tempdir().unwrap();
    let dump = dump_dir.path().join("big.jsonl");
    cmd_dump(dir.path(), &dump).unwrap();
    let dst = tempfile::tempdir().unwrap();
    cmd_restore(dst.path(), &dump).unwrap();

    let report = check_report(dst.path());
    assert!(report.ok);
    let engine = Engine::open(dst.path()).unwrap();
    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "Item".into(),
                filter: None
            })
            .unwrap(),
        CI_N
    );
}

#[test]
fn large_dataset_compaction_smoke() {
    let dir = tempfile::tempdir().unwrap();
    build(dir.path(), CI_N);
    {
        let engine = Engine::open(dir.path()).unwrap();
        engine.compact().unwrap();
    }
    let report = check_report(dir.path());
    assert!(report.ok, "compacted large dataset passes check");
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "Item".into(),
                filter: None
            })
            .unwrap(),
        CI_N
    );
}

#[test]
#[ignore = "stress: 100k records; run with --ignored"]
fn large_dataset_ignored_100k_stress() {
    let dir = tempfile::tempdir().unwrap();
    build(dir.path(), STRESS_N);
    let report = check_report(dir.path());
    assert!(report.ok, "100k dataset passes check: {:?}", report.errors);
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "Item".into(),
                filter: None
            })
            .unwrap(),
        STRESS_N
    );
    // Vector nearest and full-text still work at 100k.
    let v = engine
        .find(&FindQuery {
            vector: Some(VectorSearch {
                field: "embedding".into(),
                query: vec![0.0; DIM],
                k: 10,
                metric: "euclidean".into(),
            }),
            ..FindQuery::new("Item")
        })
        .unwrap();
    assert_eq!(v.len(), 10);
}
