//! GROUP BY aggregations (v1.3.0).
//!
//! Exercises single-field GROUP BY with per-group `count`/`min`/`max`/`avg`
//! metrics: deterministic count-desc / key-asc ordering, `group_limit`
//! truncation with an honest `group_count_total`, null/missing exclusion,
//! filter and BM25 search-candidate scoping, and restart persistence. GROUP BY
//! composes with the existing aggregate matched set, so it inherits the same
//! filter and search-scoping semantics as facets/metrics.

use std::thread::sleep;
use std::time::Duration;

use auradb::core::{
    CollectionSchema, Document, ErrorCode, FieldDef, FieldType, IndexDef, IndexKind, Value,
};
use auradb::query::{
    AggregateMetric, AggregateOp, AggregateQuery, CompareOp, Deadline, Filter, TextOperator,
    TextRank, TextSearch,
};
use auradb::Engine;

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
            name: "category".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: false,
            indexed: true,
        })
        .with_field(FieldDef::new("brand", FieldType::String))
        .with_field(FieldDef::new("price", FieldType::Int))
        // Nullable / sometimes-absent group field for the missing-values policy.
        .with_field(FieldDef {
            name: "region".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: true,
            indexed: false,
        })
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 2 }))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

/// (id, category, brand, price, region|"", body). region == "" means the field
/// is omitted entirely (absent, not null), to exercise the missing-key policy.
const ROWS: &[(usize, &str, &str, i64, &str, &str)] = &[
    (0, "a", "acme", 10, "north", "red running shoe"),
    (1, "a", "acme", 20, "north", "blue running shoe"),
    (2, "a", "bolt", 30, "north", "running shoe laces"),
    (3, "a", "bolt", 40, "north", "trail running shoe"),
    (4, "a", "cusp", 50, "south", "running sandal"),
    (5, "b", "acme", 15, "south", "winter boot"),
    (6, "b", "bolt", 25, "", "rain boot"),
    (7, "b", "cusp", 35, "", "hiking boot"),
    (8, "c", "acme", 45, "", "wool sock"),
    (9, "c", "bolt", 55, "", "cotton sock"),
];

fn doc(id: usize, category: &str, brand: &str, price: i64, region: &str, body: &str) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(format!("p{id}")));
    f.insert("category".into(), Value::Text(category.into()));
    f.insert("brand".into(), Value::Text(brand.into()));
    f.insert("price".into(), Value::Int(price));
    if !region.is_empty() {
        f.insert("region".into(), Value::Text(region.into()));
    }
    f.insert("body".into(), Value::Text(body.into()));
    f.insert("embedding".into(), Value::Vector(vec![price as f32, 1.0]));
    f
}

fn seeded(engine: &Engine) {
    engine.create_schema(schema()).unwrap();
    for (id, cat, brand, price, region, body) in ROWS.iter().copied() {
        engine
            .insert("Item", doc(id, cat, brand, price, region, body))
            .unwrap();
    }
}

fn group_by(field: &str) -> AggregateQuery {
    let mut q = AggregateQuery::new("Item");
    q.group_by = Some(field.into());
    q
}

fn metric(op: AggregateOp, field: Option<&str>) -> AggregateMetric {
    AggregateMetric {
        op,
        field: field.map(Into::into),
    }
}

#[test]
fn group_by_count_basic() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    let r = engine.aggregate(&group_by("category")).unwrap();
    let g = r.groups.expect("group_by populates groups");
    // Count-desc, key-asc: a=5, b=3, c=2.
    assert_eq!(g.field, "category");
    assert_eq!(g.group_count_total, 3);
    assert_eq!(g.groups.len(), 3);
    assert_eq!(g.groups[0].key, Value::Text("a".into()));
    assert_eq!(g.groups[0].count, 5);
    assert_eq!(g.groups[1].key, Value::Text("b".into()));
    assert_eq!(g.groups[1].count, 3);
    assert_eq!(g.groups[2].key, Value::Text("c".into()));
    assert_eq!(g.groups[2].count, 2);
    // The top-level matched count is unchanged by grouping.
    assert_eq!(r.matched, 10);
}

#[test]
fn group_by_count_with_filter() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    // Filter to brand=acme (p0,p1 in a; p5 in b; p8 in c), then group by category.
    let mut q = group_by("category");
    q.filter = Some(Filter::Compare {
        field: "brand".into(),
        op: CompareOp::Eq,
        value: Value::Text("acme".into()),
    });
    let r = engine.aggregate(&q).unwrap();
    assert!(r.filter_present);
    assert_eq!(r.matched, 4);
    let g = r.groups.unwrap();
    // a=2, then b=1, c=1 tie -> key-asc.
    assert_eq!(g.groups[0].key, Value::Text("a".into()));
    assert_eq!(g.groups[0].count, 2);
    assert_eq!(g.groups[1].key, Value::Text("b".into()));
    assert_eq!(g.groups[1].count, 1);
    assert_eq!(g.groups[2].key, Value::Text("c".into()));
    assert_eq!(g.groups[2].count, 1);
}

#[test]
fn group_by_min_max_avg_numeric() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    let mut q = group_by("category");
    q.metrics = vec![
        metric(AggregateOp::Min, Some("price")),
        metric(AggregateOp::Max, Some("price")),
        metric(AggregateOp::Avg, Some("price")),
        metric(AggregateOp::Count, None),
    ];
    let r = engine.aggregate(&q).unwrap();
    let g = r.groups.unwrap();
    // Group a: prices 10,20,30,40,50 -> min 10, max 50, avg 30.
    let a = &g.groups[0];
    assert_eq!(a.key, Value::Text("a".into()));
    assert_eq!(a.metrics[0].value, Value::Int(10), "min");
    assert_eq!(a.metrics[1].value, Value::Int(50), "max");
    assert_eq!(a.metrics[2].value, Value::Float(30.0), "avg");
    assert_eq!(a.metrics[3].value, Value::Int(5), "count");
    // Group b: 15,25,35 -> avg 25. Group c: 45,55 -> avg 50.
    assert_eq!(g.groups[1].metrics[2].value, Value::Float(25.0));
    assert_eq!(g.groups[2].metrics[2].value, Value::Float(50.0));
}

#[test]
fn group_by_limit_and_tie_break() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    // brand counts: acme=4, bolt=4, cusp=2. acme/bolt tie -> key-asc => acme first.
    let mut q = group_by("brand");
    q.group_limit = Some(2);
    let r = engine.aggregate(&q).unwrap();
    let g = r.groups.unwrap();
    assert_eq!(g.group_limit, 2);
    assert_eq!(g.group_count_total, 3, "truncation is visible via total");
    assert_eq!(g.groups.len(), 2);
    assert_eq!(g.groups[0].key, Value::Text("acme".into()));
    assert_eq!(g.groups[0].count, 4);
    assert_eq!(g.groups[1].key, Value::Text("bolt".into()));
    assert_eq!(g.groups[1].count, 4);
}

#[test]
fn group_by_missing_values_policy() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    // region present on 6 rows (north x4, south x2), absent on 4. Absent rows are
    // excluded from grouping; matched still counts the whole set.
    let r = engine.aggregate(&group_by("region")).unwrap();
    let g = r.groups.unwrap();
    assert_eq!(r.matched, 10);
    assert_eq!(g.group_count_total, 2);
    let grouped: usize = g.groups.iter().map(|b| b.count).sum();
    assert_eq!(grouped, 6, "rows with absent group key are excluded");
    assert_eq!(g.groups[0].key, Value::Text("north".into()));
    assert_eq!(g.groups[0].count, 4);
    assert_eq!(g.groups[1].key, Value::Text("south".into()));
    assert_eq!(g.groups[1].count, 2);
}

#[test]
fn group_by_search_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    // GROUP BY over the BM25 "running" candidate set (p0..p4, all category a).
    let mut q = group_by("category");
    q.text_search = Some(Box::new(TextSearch {
        field: "body".into(),
        query: "running".into(),
        operator: TextOperator::Or,
        rank: TextRank::Bm25,
        k1: None,
        b: None,
    }));
    let r = engine.aggregate(&q).unwrap();
    assert!(r.search_scoped);
    assert_eq!(r.matched, 5);
    let g = r.groups.unwrap();
    assert_eq!(g.groups.len(), 1);
    assert_eq!(g.groups[0].key, Value::Text("a".into()));
    assert_eq!(g.groups[0].count, 5);
}

#[test]
fn group_by_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seeded(&engine);
    }
    let engine = Engine::open(dir.path()).unwrap();
    let r = engine.aggregate(&group_by("category")).unwrap();
    let g = r.groups.unwrap();
    assert_eq!(g.groups[0].count, 5);
    assert_eq!(g.group_count_total, 3);
}

#[test]
fn group_by_invalid_field_and_limit_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    // Unknown group field.
    let err = engine
        .aggregate(&group_by("nonexistent"))
        .expect_err("unknown group field rejected");
    assert_eq!(err.code(), ErrorCode::InvalidRequest);

    // Non-scalar (vector) group field.
    let err = engine
        .aggregate(&group_by("embedding"))
        .expect_err("vector group field rejected");
    assert_eq!(err.code(), ErrorCode::InvalidRequest);

    // group_limit = 0 rejected.
    let mut q = group_by("category");
    q.group_limit = Some(0);
    let err = engine.aggregate(&q).expect_err("zero group_limit rejected");
    assert_eq!(err.code(), ErrorCode::InvalidRequest);
}

#[test]
fn group_by_avg_skips_non_numeric() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    // avg over a non-numeric field yields null per group (no numeric values),
    // never an error.
    let mut q = group_by("category");
    q.metrics = vec![metric(AggregateOp::Avg, Some("brand"))];
    let r = engine.aggregate(&q).unwrap();
    for bucket in r.groups.unwrap().groups {
        assert_eq!(bucket.metrics[0].value, Value::Null);
    }
}

#[test]
fn group_by_explain_shape() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    // The aggregate result is the diagnostic surface for grouped queries: it
    // reports the matched/scanned sizes, scoping flags, and the group plan
    // (field, distinct-group total, applied limit) honestly.
    let mut q = group_by("category");
    q.metrics = vec![metric(AggregateOp::Count, None)];
    let r = engine.aggregate(&q).unwrap();
    assert_eq!(r.collection, "Item");
    assert_eq!(r.scanned, 10);
    assert_eq!(r.matched, 10);
    assert!(!r.filter_present);
    assert!(!r.search_scoped);
    let g = r.groups.unwrap();
    assert_eq!(g.field, "category");
    assert_eq!(g.group_count_total, 3);
    assert_eq!(g.group_limit, auradb::query::DEFAULT_GROUP_LIMIT);
}

#[test]
fn group_by_redacts_payload() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    // A filter value must not be echoed verbatim in the serialized result/diagnostics.
    let mut q = group_by("category");
    q.filter = Some(Filter::Compare {
        field: "brand".into(),
        op: CompareOp::Eq,
        value: Value::Text("acme-secret-token".into()),
    });
    let r = engine.aggregate(&q).unwrap();
    let json = serde_json::to_string(&r).unwrap();
    assert!(
        !json.contains("acme-secret-token"),
        "the filter payload must not appear in the result"
    );
}

#[test]
fn group_by_honors_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine);

    let q = group_by("brand");
    let deadline = Deadline::after_ms(1);
    sleep(Duration::from_millis(6));
    let err = engine
        .aggregate_within(&q, &deadline)
        .expect_err("group_by must time out under an expired deadline");
    assert_eq!(err.code(), ErrorCode::QueryTimeout);

    let ok = engine
        .aggregate_within(&q, &Deadline::after_ms(60_000))
        .expect("engine usable after a group_by timeout");
    assert_eq!(ok.groups.unwrap().group_count_total, 3);
}
