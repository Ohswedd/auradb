//! Query planner and `EXPLAIN ANALYZE` integration tests at the engine level:
//! costed index selection, persisted statistics, the plan tree, and measured
//! execution metrics.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{Access, CompareOp, Filter, FindQuery, PlanNode, Strategy, VectorSearch};
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
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
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
    m.insert("body".into(), Value::Text("alpha beta gamma".into()));
    m.insert("embedding".into(), Value::Vector(vec![1.0, 0.0, 0.0]));
    m
}

fn eq(field: &str, value: Value) -> Filter {
    Filter::Compare {
        field: field.into(),
        op: CompareOp::Eq,
        value,
    }
}

#[test]
fn explain_reports_plan_tree_and_strategy() {
    let (_d, engine) = open();
    for i in 0..20 {
        engine
            .insert(
                "Doc",
                doc(&format!("d{i}"), if i < 2 { "rare" } else { "common" }),
            )
            .unwrap();
    }
    engine.analyze().unwrap();

    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("id", Value::Text("d1".into())));
    let plan = engine.explain(&q).unwrap();
    assert_eq!(plan.strategy, Strategy::IndexLookup);
    assert_eq!(plan.used_index.as_deref(), Some("id"));
    // The plan tree is present: Filter wrapping a PointLookup.
    let tree = plan.plan_tree.expect("plan tree present");
    match tree {
        PlanNode::Filter { input, .. } => {
            assert!(matches!(*input, PlanNode::PointLookup { .. }), "{input:?}");
        }
        other => panic!("expected filter root, got {other:?}"),
    }
}

#[test]
fn planner_uses_secondary_index_after_analyze() {
    let (_d, engine) = open();
    for i in 0..50 {
        engine
            .insert(
                "Doc",
                doc(&format!("d{i}"), if i == 0 { "rare" } else { "common" }),
            )
            .unwrap();
    }
    engine.analyze().unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("status", Value::Text("rare".into())));
    let plan = engine.explain(&q).unwrap();
    assert_eq!(plan.strategy, Strategy::IndexLookup);
    assert_eq!(plan.used_index.as_deref(), Some("status"));
}

#[test]
fn stats_persist_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        engine.create_schema(schema()).unwrap();
        for i in 0..30 {
            engine.insert("Doc", doc(&format!("d{i}"), "s")).unwrap();
        }
        engine.analyze().unwrap();
        let stats = engine.planner_stats();
        assert_eq!(stats.get("Doc").unwrap().row_count, 30);
    }
    // Reopen: persisted stats are loaded, not recomputed from scratch.
    let engine = Engine::open(dir.path()).unwrap();
    let stats = engine.planner_stats();
    assert_eq!(stats.get("Doc").unwrap().row_count, 30);
    assert!(stats.get("Doc").unwrap().cardinality("status").is_some());
}

#[test]
fn explain_analyze_point_lookup_reports_actual_rows() {
    let (_d, engine) = open();
    for i in 0..10 {
        engine.insert("Doc", doc(&format!("d{i}"), "s")).unwrap();
    }
    engine.analyze().unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("id", Value::Text("d3".into())));
    let plan = engine.explain_analyze(&q).unwrap();
    let a = plan.analysis.expect("analysis present");
    assert_eq!(a.scanned_rows, 1);
    assert_eq!(a.matched_rows, 1);
    assert_eq!(a.returned_rows, 1);
    assert_eq!(a.snapshot_ts, None);
}

#[test]
fn explain_analyze_index_lookup_reports_actual_rows() {
    let (_d, engine) = open();
    for i in 0..10 {
        engine
            .insert("Doc", doc(&format!("d{i}"), if i < 3 { "x" } else { "y" }))
            .unwrap();
    }
    engine.analyze().unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("status", Value::Text("x".into())));
    let plan = engine.explain_analyze(&q).unwrap();
    assert_eq!(plan.strategy, Strategy::IndexLookup);
    let a = plan.analysis.unwrap();
    assert_eq!(a.matched_rows, 3);
    assert_eq!(a.returned_rows, 3);
}

#[test]
fn explain_analyze_scan_reports_actual_rows() {
    let (_d, engine) = open();
    for i in 0..8 {
        engine.insert("Doc", doc(&format!("d{i}"), "s")).unwrap();
    }
    let plan = engine.explain_analyze(&FindQuery::new("Doc")).unwrap();
    assert_eq!(plan.strategy, Strategy::FullScan);
    let a = plan.analysis.unwrap();
    assert_eq!(a.scanned_rows, 8);
    assert_eq!(a.returned_rows, 8);
}

#[test]
fn explain_analyze_full_text() {
    let (_d, engine) = open();
    for i in 0..5 {
        engine.insert("Doc", doc(&format!("d{i}"), "s")).unwrap();
    }
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::ContainsText {
        field: "body".into(),
        query: "alpha".into(),
    });
    let plan = engine.explain_analyze(&q).unwrap();
    assert_eq!(plan.strategy, Strategy::FullTextScan);
    assert_eq!(plan.analysis.unwrap().matched_rows, 5);
}

#[test]
fn explain_analyze_vector() {
    let (_d, engine) = open();
    for i in 0..5 {
        engine.insert("Doc", doc(&format!("d{i}"), "s")).unwrap();
    }
    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![1.0, 0.0, 0.0],
        k: 3,
        metric: "cosine".into(),
    });
    let plan = engine.explain_analyze(&q).unwrap();
    assert_eq!(plan.strategy, Strategy::VectorExactScan);
    let a = plan.analysis.unwrap();
    assert_eq!(a.returned_rows, 3);
}

#[test]
fn explain_analyze_inside_transaction_reports_snapshot() {
    let (_d, engine) = open();
    engine.insert("Doc", doc("d1", "s")).unwrap();
    let txn = engine.begin();
    let plan = engine
        .txn_explain_analyze(&txn, &FindQuery::new("Doc"))
        .unwrap();
    let a = plan.analysis.unwrap();
    assert_eq!(a.snapshot_ts, Some(txn.read_ts()));
}

#[test]
fn access_enum_is_exported_and_usable() {
    // The planner Access type is part of the public query surface.
    let scan = Access::Scan;
    assert_eq!(scan.used_index(), None);
}
