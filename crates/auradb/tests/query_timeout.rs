//! Cooperative query-timeout enforcement (v1.2.0).
//!
//! The engine bounds read execution with a cooperative [`Deadline`]: scan, BM25,
//! hybrid, and exact-vector reads poll it and abandon work with a structured
//! `query_timeout` error once the wall-clock budget is exceeded. These tests
//! drive a *real* over-budget deadline deterministically — the deadline's clock
//! starts before a short sleep, so the first cooperative check is always past
//! the 1ms budget regardless of host speed — and confirm the engine and its data
//! remain fully usable afterwards (the timeout cancels a query, not the session).

use std::thread::sleep;
use std::time::Duration;

use auradb::core::{
    CollectionSchema, Document, ErrorCode, FieldDef, FieldType, IndexDef, IndexKind, Value,
};
use auradb::query::{
    Deadline, FindQuery, FusionMode, HybridSearch, HybridWeights, TextOperator, TextRank,
    TextSearch, VectorSearch,
};
use auradb::Engine;

const DIM: usize = 3;

fn schema() -> CollectionSchema {
    CollectionSchema::new("Doc")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("category", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: DIM }))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

fn doc(id: usize) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(format!("d{id}")));
    f.insert(
        "body".into(),
        Value::Text("raft consensus replicates the log across nodes".into()),
    );
    f.insert("category".into(), Value::Text(format!("c{}", id % 5)));
    let a = (id % 7) as f32;
    f.insert(
        "embedding".into(),
        Value::Vector(vec![a, (id % 3) as f32, 1.0]),
    );
    f
}

fn seeded_engine(dir: &std::path::Path, n: usize) -> Engine {
    let engine = Engine::open(dir).unwrap();
    engine.create_schema(schema()).unwrap();
    for i in 0..n {
        engine.insert("Doc", doc(i)).unwrap();
    }
    engine
}

/// A deadline whose 1ms budget is already spent: constructing it starts the
/// clock, and the caller sleeps past the budget before the engine's first
/// cooperative check, making the timeout deterministic on any host.
fn expired_deadline() -> Deadline {
    let d = Deadline::after_ms(1);
    sleep(Duration::from_millis(6));
    d
}

fn scan_query() -> FindQuery {
    // No index seeds a full scan: the dominant cost path the deadline guards.
    FindQuery::new("Doc")
}

fn bm25_query() -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.text_search = Some(Box::new(TextSearch {
        field: "body".into(),
        query: "raft consensus".into(),
        operator: TextOperator::Or,
        rank: TextRank::Bm25,
        k1: None,
        b: None,
        analyzer: None,
    }));
    q
}

fn vector_query() -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![1.0, 1.0, 1.0],
        k: 5,
        metric: "cosine".into(),
    });
    q
}

fn hybrid_query() -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.hybrid = Some(Box::new(HybridSearch {
        text_field: "body".into(),
        text_query: "raft consensus".into(),
        vector_field: "embedding".into(),
        vector: vec![1.0, 1.0, 1.0],
        top_k: 5,
        metric: None,
        weights: HybridWeights::default(),
        fusion: FusionMode::WeightedSum,
        operator: TextOperator::Or,
        k1: None,
        b: None,
        analyzer: None,
    }));
    q
}

fn assert_times_out(engine: &Engine, q: &FindQuery, label: &str) {
    // `PlannedFind` is not `Debug`, so unwrap the error explicitly rather than
    // via `expect_err` (which would need to format the Ok value).
    let err = engine
        .plan_find_within(q, &expired_deadline())
        .err()
        .unwrap_or_else(|| panic!("{label} query must time out under an expired deadline"));
    assert_eq!(
        err.code(),
        ErrorCode::QueryTimeout,
        "{label}: expected query_timeout, got {err}"
    );
}

#[test]
fn query_timeout_scan() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded_engine(dir.path(), 200);
    assert_times_out(&engine, &scan_query(), "scan");
}

#[test]
fn query_timeout_bm25() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded_engine(dir.path(), 200);
    assert_times_out(&engine, &bm25_query(), "bm25");
}

#[test]
fn query_timeout_vector() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded_engine(dir.path(), 200);
    assert_times_out(&engine, &vector_query(), "vector");
}

#[test]
fn query_timeout_hybrid() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded_engine(dir.path(), 200);
    assert_times_out(&engine, &hybrid_query(), "hybrid");
}

#[test]
fn query_timeout_error_shape() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded_engine(dir.path(), 50);
    let err = engine
        .plan_find_within(&scan_query(), &expired_deadline())
        .err()
        .expect("must time out");
    // Stable, machine-readable code plus an honest message that names the budget.
    assert_eq!(err.code(), ErrorCode::QueryTimeout);
    assert_eq!(err.code().as_str(), "query_timeout");
    let msg = err.to_string();
    assert!(
        msg.contains("deadline"),
        "message names the deadline: {msg}"
    );
}

#[test]
fn query_timeout_connection_survives() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded_engine(dir.path(), 100);

    // A query that times out must not poison the engine: the same engine still
    // serves subsequent reads and writes correctly.
    assert_times_out(&engine, &scan_query(), "scan");

    // A generous deadline on the very same query now succeeds.
    let ok = engine
        .plan_find_within(&scan_query(), &Deadline::after_ms(60_000))
        .expect("engine remains usable after a timeout");
    assert_eq!(ok.ordered.len(), 100, "all rows returned within budget");

    // And ordinary, deadline-free reads/writes keep working.
    engine.insert("Doc", doc(100)).unwrap();
    let rows = engine.find(&scan_query()).unwrap();
    assert_eq!(rows.len(), 101, "write after timeout is visible");

    // A disabled deadline (0) never trips, even on the full scan.
    engine
        .plan_find_within(&scan_query(), &Deadline::after_ms(0))
        .expect("a disabled deadline never fires");
}

#[test]
fn generous_timeout_does_not_false_trip() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded_engine(dir.path(), 300);
    // The per-query IR timeout path: a comfortable budget returns full results.
    let mut q = scan_query();
    q.timeout_ms = Some(60_000);
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 300);
}
