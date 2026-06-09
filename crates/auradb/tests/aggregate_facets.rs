//! Aggregations and terms facets (v1.2.0).
//!
//! Exercises `count`/`min`/`max` metrics and terms facets over the engine:
//! index-backed vs scan facet paths, deterministic count-desc / value-asc bucket
//! ordering and `limit` truncation, residual-filter and BM25 search-facet
//! scoping, restart persistence, and invalid-field rejection.

use std::thread::sleep;
use std::time::Duration;

use auradb::core::{
    CollectionSchema, Document, ErrorCode, FieldDef, FieldType, IndexDef, IndexKind, Value,
};
use auradb::query::{
    AggregateMetric, AggregateOp, AggregateQuery, CompareOp, Deadline, FacetRequest, Filter,
    TextOperator, TextRank, TextSearch,
};
use auradb::Engine;

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
        // `category` carries a secondary equality index -> index-backed facets.
        .with_field(FieldDef {
            name: "category".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: false,
            indexed: true,
        })
        // `brand` is unindexed -> scan-path facets.
        .with_field(FieldDef::new("brand", FieldType::String))
        .with_field(FieldDef::new("price", FieldType::Int))
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 2 }))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

fn doc(id: usize, category: &str, brand: &str, price: i64, body: &str) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(format!("p{id}")));
    f.insert("category".into(), Value::Text(category.into()));
    f.insert("brand".into(), Value::Text(brand.into()));
    f.insert("price".into(), Value::Int(price));
    f.insert("body".into(), Value::Text(body.into()));
    f.insert("embedding".into(), Value::Vector(vec![price as f32, 1.0]));
    f
}

/// Seed a deterministic catalog: category counts a=5, b=3, c=2 (count-desc
/// order), with brands chosen so two categories tie for ordering tests.
fn seeded(engine: &Engine) {
    engine.create_schema(schema()).unwrap();
    let rows = [
        (0, "a", "acme", 10, "red running shoe"),
        (1, "a", "acme", 20, "blue running shoe"),
        (2, "a", "bolt", 30, "running shoe laces"),
        (3, "a", "bolt", 40, "trail running shoe"),
        (4, "a", "cusp", 50, "running sandal"),
        (5, "b", "acme", 15, "winter boot"),
        (6, "b", "bolt", 25, "rain boot"),
        (7, "b", "cusp", 35, "hiking boot"),
        (8, "c", "acme", 45, "wool sock"),
        (9, "c", "bolt", 55, "cotton sock"),
    ];
    for (id, cat, brand, price, body) in rows {
        engine
            .insert("Product", doc(id, cat, brand, price, body))
            .unwrap();
    }
}

fn count_metric() -> AggregateMetric {
    AggregateMetric {
        op: AggregateOp::Count,
        field: None,
    }
}

fn facet(field: &str, limit: Option<usize>) -> FacetRequest {
    FacetRequest {
        field: field.into(),
        limit,
    }
}

#[test]
fn aggregate_count_all() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);
    let mut q = AggregateQuery::new("Product");
    q.metrics = vec![count_metric()];
    let r = engine.aggregate(&q).unwrap();
    assert_eq!(r.matched, 10);
    assert_eq!(r.metrics[0].value, Value::Int(10));
    assert_eq!(r.metrics[0].op, "count");
}

#[test]
fn aggregate_count_with_filter() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);
    let mut q = AggregateQuery::new("Product");
    q.filter = Some(Filter::Compare {
        field: "category".into(),
        op: CompareOp::Eq,
        value: Value::Text("a".into()),
    });
    q.metrics = vec![count_metric()];
    let r = engine.aggregate(&q).unwrap();
    assert_eq!(r.matched, 5);
    assert_eq!(r.metrics[0].value, Value::Int(5));
    assert!(r.filter_present);
}

#[test]
fn aggregate_min_max() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);
    let mut q = AggregateQuery::new("Product");
    q.metrics = vec![
        AggregateMetric {
            op: AggregateOp::Min,
            field: Some("price".into()),
        },
        AggregateMetric {
            op: AggregateOp::Max,
            field: Some("price".into()),
        },
    ];
    let r = engine.aggregate(&q).unwrap();
    assert_eq!(r.metrics[0].value, Value::Int(10), "min price");
    assert_eq!(r.metrics[1].value, Value::Int(55), "max price");
}

#[test]
fn facet_terms_counts() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);
    let mut q = AggregateQuery::new("Product");
    q.facets = vec![facet("category", None)];
    let r = engine.aggregate(&q).unwrap();
    let buckets = &r.facets[0].buckets;
    // Count-desc, value-asc: a=5, b=3, c=2.
    assert_eq!(buckets.len(), 3);
    assert_eq!(buckets[0].value, Value::Text("a".into()));
    assert_eq!(buckets[0].count, 5);
    assert_eq!(buckets[1].value, Value::Text("b".into()));
    assert_eq!(buckets[1].count, 3);
    assert_eq!(buckets[2].value, Value::Text("c".into()));
    assert_eq!(buckets[2].count, 2);
}

#[test]
fn facet_terms_limit_and_tie_break() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);
    // brand counts: acme=4, bolt=4, cusp=2. acme/bolt tie -> value-asc => acme first.
    let mut q = AggregateQuery::new("Product");
    q.facets = vec![facet("brand", Some(2))];
    let r = engine.aggregate(&q).unwrap();
    let buckets = &r.facets[0].buckets;
    assert_eq!(buckets.len(), 2, "limit truncates to 2 buckets");
    assert_eq!(buckets[0].value, Value::Text("acme".into()));
    assert_eq!(buckets[0].count, 4);
    assert_eq!(buckets[1].value, Value::Text("bolt".into()));
    assert_eq!(buckets[1].count, 4);
    assert!(!r.facets[0].used_index, "brand is unindexed -> scan path");
}

#[test]
fn facet_uses_index_when_available() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);
    // No filter, no search scope, indexed field -> index-backed facet.
    let mut q = AggregateQuery::new("Product");
    q.facets = vec![facet("category", None)];
    let r = engine.aggregate(&q).unwrap();
    assert!(
        r.facets[0].used_index,
        "indexed category uses the index path"
    );
    // Counts must still be correct from the index posting lengths.
    assert_eq!(r.facets[0].buckets[0].count, 5);
}

#[test]
fn facet_scan_fallback_correct() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);
    // A residual filter forces the scan path even for the indexed field, and the
    // counts must reflect only the filtered set.
    let mut q = AggregateQuery::new("Product");
    q.filter = Some(Filter::Compare {
        field: "brand".into(),
        op: CompareOp::Eq,
        value: Value::Text("acme".into()),
    });
    q.facets = vec![facet("category", None)];
    let r = engine.aggregate(&q).unwrap();
    assert!(!r.facets[0].used_index, "filter forces scan fallback");
    // acme appears in a(2: p0,p1), b(1: p5), c(1: p8).
    let buckets = &r.facets[0].buckets;
    assert_eq!(buckets[0].value, Value::Text("a".into()));
    assert_eq!(buckets[0].count, 2);
    // b and c tie at 1 -> value-asc.
    assert_eq!(buckets[1].value, Value::Text("b".into()));
    assert_eq!(buckets[2].value, Value::Text("c".into()));
}

#[test]
fn search_with_facets_bm25() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);
    // Facet the "running" BM25 candidate set: docs p0..p4 (category a) mention
    // "running"; the facet over category should reflect that candidate set.
    let mut q = AggregateQuery::new("Product");
    q.text_search = Some(Box::new(TextSearch {
        field: "body".into(),
        query: "running".into(),
        operator: TextOperator::Or,
        rank: TextRank::Bm25,
        k1: None,
        b: None,
    }));
    q.facets = vec![facet("category", None)];
    q.metrics = vec![count_metric()];
    let r = engine.aggregate(&q).unwrap();
    assert!(r.search_scoped);
    assert_eq!(r.matched, 5, "five 'running' documents");
    assert_eq!(r.facets[0].buckets.len(), 1);
    assert_eq!(r.facets[0].buckets[0].value, Value::Text("a".into()));
    assert_eq!(r.facets[0].buckets[0].count, 5);
    assert!(!r.facets[0].used_index, "search scope forces the scan path");
}

#[test]
fn facet_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seeded(&engine);
    }
    // Reopen: facets must recompute correctly from persisted data + indexes.
    let engine = Engine::open(dir.path()).unwrap();
    let mut q = AggregateQuery::new("Product");
    q.facets = vec![facet("category", None)];
    let r = engine.aggregate(&q).unwrap();
    assert_eq!(r.facets[0].buckets[0].count, 5);
    assert!(r.facets[0].used_index, "index rebuilt on open");
}

#[test]
fn facet_explain_shape() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);
    let mut q = AggregateQuery::new("Product");
    q.facets = vec![facet("category", None)];
    q.metrics = vec![count_metric()];
    let r = engine.aggregate(&q).unwrap();
    // The result carries diagnostic shape: collection, matched/scanned, the
    // scoping flags, and per-facet index/scan provenance.
    assert_eq!(r.collection, "Product");
    assert_eq!(r.scanned, 10);
    assert_eq!(r.matched, 10);
    assert!(!r.filter_present);
    assert!(!r.search_scoped);
    assert_eq!(r.facets[0].field, "category");
}

#[test]
fn facet_invalid_field_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    // Unknown field.
    let mut q = AggregateQuery::new("Product");
    q.facets = vec![facet("nonexistent", None)];
    let err = engine.aggregate(&q).expect_err("unknown field rejected");
    assert_eq!(err.code(), auradb::core::ErrorCode::InvalidRequest);

    // Non-scalar (vector) field.
    let mut q = AggregateQuery::new("Product");
    q.facets = vec![facet("embedding", None)];
    let err = engine.aggregate(&q).expect_err("vector facet rejected");
    assert_eq!(err.code(), auradb::core::ErrorCode::InvalidRequest);

    // Empty aggregate (no facets, no metrics) is rejected.
    let q = AggregateQuery::new("Product");
    let err = engine.aggregate(&q).expect_err("empty aggregate rejected");
    assert_eq!(err.code(), auradb::core::ErrorCode::InvalidRequest);
}

#[test]
fn query_timeout_facets() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    // An aggregate/facet query also honors the cooperative deadline. The 1ms
    // budget is spent before the engine's first check (the clock starts before
    // the sleep), so the timeout is deterministic on any host.
    let mut q = AggregateQuery::new("Product");
    q.facets = vec![facet("brand", None)]; // scan-path facet polls the deadline
    q.metrics = vec![count_metric()];
    let deadline = Deadline::after_ms(1);
    sleep(Duration::from_millis(6));
    let err = engine
        .aggregate_within(&q, &deadline)
        .expect_err("aggregate must time out under an expired deadline");
    assert_eq!(err.code(), ErrorCode::QueryTimeout);

    // A generous budget on the same query returns full results — the engine is
    // unaffected by the prior timeout.
    let ok = engine
        .aggregate_within(&q, &Deadline::after_ms(60_000))
        .expect("engine usable after an aggregate timeout");
    assert_eq!(ok.matched, 10);
}
