//! EXPLAIN ANALYZE observability fields (v0.3.1).
//!
//! v0.3.1 enriches `EXPLAIN ANALYZE` with the planner-stats version, the
//! estimated-vs-actual row counts, a human-readable index-selection reason, the
//! MVCC snapshot timestamp, and a stale-statistics warning. All additions are
//! additive JSON fields so Aura Connector 0.3.x stays compatible.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{CompareOp, Filter, FindQuery};
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
            name: "status".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: true,
            indexed: true,
        })
}

fn open() -> (tempfile::TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    (dir, engine)
}

fn doc(id: &str, status: &str) -> Document {
    let mut m = Document::new();
    m.insert("id".into(), Value::Text(id.into()));
    m.insert("status".into(), Value::Text(status.into()));
    m
}

fn eq(field: &str, value: &str) -> Filter {
    Filter::Compare {
        field: field.into(),
        op: CompareOp::Eq,
        value: Value::Text(value.into()),
    }
}

#[test]
fn explain_analyze_reports_snapshot_timestamp() {
    let (_d, engine) = open();
    engine.insert("Doc", doc("d1", "a")).unwrap();
    let txn = engine.begin();
    let plan = engine
        .txn_explain_analyze(&txn, &FindQuery::new("Doc"))
        .unwrap();
    let a = plan.analysis.unwrap();
    assert_eq!(a.snapshot_ts, Some(txn.read_ts()));
    // A non-transactional ANALYZE has no snapshot timestamp.
    let plan = engine.explain_analyze(&FindQuery::new("Doc")).unwrap();
    assert_eq!(plan.analysis.unwrap().snapshot_ts, None);
    engine.rollback(txn);
}

#[test]
fn explain_analyze_reports_estimated_and_actual_rows() {
    let (_d, engine) = open();
    for i in 0..10 {
        engine
            .insert("Doc", doc(&format!("d{i}"), if i < 3 { "x" } else { "y" }))
            .unwrap();
    }
    engine.analyze().unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("status", "x"));
    let plan = engine.explain_analyze(&q).unwrap();
    let a = plan.analysis.unwrap();
    // Estimate is carried alongside the measured actuals.
    assert_eq!(a.estimated_rows, plan.estimated_rows);
    assert_eq!(a.matched_rows, 3);
    assert_eq!(a.returned_rows, 3);
}

#[test]
fn explain_analyze_reports_selected_index_reason() {
    let (_d, engine) = open();
    for i in 0..10 {
        engine.insert("Doc", doc(&format!("d{i}"), "x")).unwrap();
    }
    engine.analyze().unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("id", "d1"));
    let plan = engine.explain_analyze(&q).unwrap();
    let a = plan.analysis.unwrap();
    let reason = a.selected_index_reason.unwrap();
    assert!(reason.contains("id"), "{reason}");
    assert!(a.planner_stats_version.is_some());
}

#[test]
fn explain_analyze_warns_on_stale_stats() {
    let (_d, engine) = open();
    // Insert rows but never run `analyze`: the per-field cardinality is empty, so
    // the planner used defaults and ANALYZE flags the statistics as stale.
    for i in 0..10 {
        engine.insert("Doc", doc(&format!("d{i}"), "x")).unwrap();
    }
    let plan = engine.explain_analyze(&FindQuery::new("Doc")).unwrap();
    assert!(plan.analysis.unwrap().stale_stats);
    assert!(plan.warnings.iter().any(|w| w.contains("statistics")));

    // After analyze the warning clears.
    engine.analyze().unwrap();
    let plan = engine.explain_analyze(&FindQuery::new("Doc")).unwrap();
    assert!(!plan.analysis.unwrap().stale_stats);
}

#[test]
fn explain_analyze_shape_stable_for_connector() {
    // The ANALYZE object must round-trip through JSON with all v0.3.0 fields
    // present and unchanged; v0.3.1 only adds fields. A 0.3.x connector that
    // models the old shape still deserializes it.
    let (_d, engine) = open();
    engine.insert("Doc", doc("d1", "a")).unwrap();
    engine.analyze().unwrap();
    let plan = engine.explain_analyze(&FindQuery::new("Doc")).unwrap();
    let json = serde_json::to_value(&plan).unwrap();
    let analysis = json.get("analysis").expect("analysis present");
    for field in [
        "scanned_rows",
        "matched_rows",
        "returned_rows",
        "execution_micros",
        "planning_micros",
    ] {
        assert!(
            analysis.get(field).is_some(),
            "missing v0.3.0 field {field}"
        );
    }
    // Additive v0.3.1 fields.
    assert!(analysis.get("estimated_rows").is_some());
    assert!(analysis.get("selected_index_reason").is_some());
    // The plan still deserializes back into the strongly-typed shape.
    let back: auradb::query::ExplainPlan = serde_json::from_value(json).unwrap();
    assert_eq!(back.collection, "Doc");
}
