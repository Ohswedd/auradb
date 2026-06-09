//! Backup/restore coverage for the v1.2.0 query features (aggregations, terms
//! facets, query timeouts).
//!
//! The new features compute over existing records and the existing equality /
//! full-text indexes — nothing new is persisted — so the contract to verify is
//! that an `auradb dump` → `auradb restore` round-trip rebuilds those indexes and
//! the aggregate/facet/timeout paths produce identical results on the restored
//! database. The index-backed facet path in particular only reports
//! `used_index = true` if the equality index rebuilt correctly on restore.

use std::thread::sleep;
use std::time::Duration;

use auradb::core::{
    CollectionSchema, Document, ErrorCode, FieldDef, FieldType, IndexDef, IndexKind, Value,
};
use auradb::query::{
    AggregateMetric, AggregateOp, AggregateQuery, Deadline, FacetRequest, TextOperator, TextRank,
    TextSearch,
};
use auradb::Engine;
use auradb_cli::{cmd_check, cmd_dump, cmd_restore};

fn schema() -> CollectionSchema {
    CollectionSchema::new("Product")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef {
            name: "category".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: false,
            indexed: true, // secondary equality index -> index-backed facets
        })
        .with_field(FieldDef::new("price", FieldType::Int))
        .with_field(FieldDef::new("body", FieldType::String))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

fn build_source(dir: &std::path::Path) {
    let engine = Engine::open(dir).unwrap();
    engine.create_schema(schema()).unwrap();
    let rows = [
        (0, "a", 10, "red running shoe"),
        (1, "a", 20, "blue running shoe"),
        (2, "a", 30, "running shoe laces"),
        (3, "b", 40, "winter boot"),
        (4, "b", 50, "rain boot"),
        (5, "c", 60, "wool sock"),
    ];
    for (id, cat, price, body) in rows {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("p{id}")));
        f.insert("category".into(), Value::Text(cat.into()));
        f.insert("price".into(), Value::Int(price));
        f.insert("body".into(), Value::Text(body.into()));
        engine.insert("Product", f).unwrap();
    }
}

/// dump `src` then restore into a fresh dir and return its opened engine.
fn dump_and_restore(src: &std::path::Path, tmp: &std::path::Path) -> Engine {
    let dump = tmp.join("backup.jsonl");
    let dst = tmp.join("restored");
    cmd_dump(src, &dump).unwrap();
    cmd_restore(&dst, &dump).unwrap();
    // `auradb check` passes on the restored database (indexes rebuilt).
    assert!(cmd_check(&dst).unwrap().contains("OK"));
    Engine::open(&dst).unwrap()
}

fn category_facet() -> AggregateQuery {
    let mut q = AggregateQuery::new("Product");
    q.facets = vec![FacetRequest {
        field: "category".into(),
        limit: None,
    }];
    q
}

#[test]
fn facets_backup_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    build_source(&src);

    // Baseline on the source.
    let before = Engine::open(&src)
        .unwrap()
        .aggregate(&category_facet())
        .unwrap();

    let engine = dump_and_restore(&src, tmp.path());
    let after = engine.aggregate(&category_facet()).unwrap();

    assert_eq!(after.facets, before.facets, "facet buckets survive restore");
    // a=3, b=2, c=1 by count desc.
    let buckets = &after.facets[0].buckets;
    assert_eq!(buckets[0].value, Value::Text("a".into()));
    assert_eq!(buckets[0].count, 3);
    // The equality index rebuilt on restore, so the facet uses the index path.
    assert!(
        after.facets[0].used_index,
        "category equality index must rebuild on restore -> index-backed facet"
    );
}

#[test]
fn aggregations_backup_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    build_source(&src);

    let mut q = AggregateQuery::new("Product");
    q.metrics = vec![
        AggregateMetric {
            op: AggregateOp::Count,
            field: None,
        },
        AggregateMetric {
            op: AggregateOp::Min,
            field: Some("price".into()),
        },
        AggregateMetric {
            op: AggregateOp::Max,
            field: Some("price".into()),
        },
    ];

    let before = Engine::open(&src).unwrap().aggregate(&q).unwrap();
    let engine = dump_and_restore(&src, tmp.path());
    let after = engine.aggregate(&q).unwrap();

    assert_eq!(after.metrics, before.metrics, "metrics survive restore");
    assert_eq!(after.metrics[0].value, Value::Int(6)); // count
    assert_eq!(after.metrics[1].value, Value::Int(10)); // min price
    assert_eq!(after.metrics[2].value, Value::Int(60)); // max price
}

#[test]
fn search_facets_backup_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    build_source(&src);

    let mut q = AggregateQuery::new("Product");
    q.text_search = Some(Box::new(TextSearch {
        field: "body".into(),
        query: "running".into(),
        operator: TextOperator::Or,
        rank: TextRank::Bm25,
        k1: None,
        b: None,
    }));
    q.facets = vec![FacetRequest {
        field: "category".into(),
        limit: None,
    }];

    let engine = dump_and_restore(&src, tmp.path());
    let after = engine.aggregate(&q).unwrap();
    // The full-text index rebuilt on restore: the three "running" docs are all
    // category "a".
    assert!(after.search_scoped);
    assert_eq!(after.matched, 3);
    assert_eq!(after.facets[0].buckets.len(), 1);
    assert_eq!(after.facets[0].buckets[0].value, Value::Text("a".into()));
    assert_eq!(after.facets[0].buckets[0].count, 3);
}

#[test]
fn query_timeout_after_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    build_source(&src);
    let engine = dump_and_restore(&src, tmp.path());

    // A generous deadline returns full results on the restored database.
    let q = category_facet();
    let ok = engine.aggregate(&q).unwrap();
    assert_eq!(ok.matched, 6);

    // The cooperative deadline still enforces after a restore.
    let deadline = Deadline::after_ms(1);
    sleep(Duration::from_millis(6));
    let err = engine
        .aggregate_within(&q, &deadline)
        .expect_err("aggregate times out after restore too");
    assert_eq!(err.code(), ErrorCode::QueryTimeout);
}
