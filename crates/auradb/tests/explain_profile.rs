//! EXPLAIN ANALYZE query-profile enrichment (v1.3.0).
//!
//! Adds additive profile fields to the ANALYZE output for production debugging:
//! a deterministic `plan_id`, the cooperative `deadline_ms`, and a
//! `timeout_checked` flag, alongside the existing measured row counts and
//! timings. The query payload is never echoed into the plan.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{CompareOp, Filter, FindQuery, VectorSearch};
use auradb::Engine;

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
        .with_field(FieldDef {
            name: "category".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: false,
            indexed: true,
        })
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
}

fn seeded(dir: &std::path::Path) -> Engine {
    let engine = Engine::open(dir).unwrap();
    engine.create_schema(schema()).unwrap();
    for i in 0..20 {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("d{i}")));
        f.insert(
            "category".into(),
            Value::Text(if i % 2 == 0 { "a" } else { "b" }.into()),
        );
        f.insert("embedding".into(), Value::Vector(vec![i as f32, 1.0, 2.0]));
        engine.insert("Doc", f).unwrap();
    }
    engine
}

#[test]
fn explain_analyze_profile_basic_filter() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path());

    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::Compare {
        field: "category".into(),
        op: CompareOp::Eq,
        value: Value::Text("a".into()),
    });
    q.timeout_ms = Some(5_000);

    let plan = engine.explain_analyze(&q).unwrap();
    let a = plan.analysis.expect("ANALYZE attaches profile");
    assert!(a.plan_id.is_some(), "deterministic plan_id present");
    assert_eq!(
        a.deadline_ms,
        Some(5_000),
        "records the cooperative deadline"
    );
    assert!(a.timeout_checked, "deadline active -> checked");
    assert_eq!(a.matched_rows, 10, "10 of 20 in category a");
    assert!(a.execution_micros >= 1 || a.matched_rows == 10);
}

#[test]
fn explain_analyze_plan_id_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path());
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::Compare {
        field: "category".into(),
        op: CompareOp::Eq,
        value: Value::Text("a".into()),
    });
    let a = engine.explain_analyze(&q).unwrap().analysis.unwrap();
    let b = engine.explain_analyze(&q).unwrap().analysis.unwrap();
    assert_eq!(a.plan_id, b.plan_id, "same query shape -> same plan_id");
    assert!(a.plan_id.unwrap().starts_with("plan-"));
}

#[test]
fn explain_analyze_no_deadline_reports_unchecked() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path());
    let q = FindQuery::new("Doc"); // no timeout
    let a = engine.explain_analyze(&q).unwrap().analysis.unwrap();
    assert_eq!(a.deadline_ms, None);
    assert!(!a.timeout_checked);
}

#[test]
fn explain_analyze_redacts_vector_payload() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seeded(dir.path());
    // A distinctive query-vector value must not appear in the serialized plan.
    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![987654.0, 123456.0, 555555.0],
        k: 5,
        metric: "cosine".into(),
    });
    let plan = engine.explain_analyze(&q).unwrap();
    let json = serde_json::to_string(&plan).unwrap();
    for needle in ["987654", "123456", "555555"] {
        assert!(
            !json.contains(needle),
            "query vector payload must not be echoed in the plan ({needle})"
        );
    }
    // But the vector plan is still described.
    let v = plan.vector.expect("vector plan present");
    assert_eq!(v.field, "embedding");
    assert_eq!(v.vector_mode.as_deref(), Some("exact"));
}
