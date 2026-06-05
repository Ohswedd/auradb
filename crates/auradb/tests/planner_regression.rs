//! Query planner regression tests (v0.3.1).
//!
//! These lock in v0.3.0 planner behaviour and the EXPLAIN ANALYZE shape. The
//! overriding contract is correctness: whatever access path the planner chooses,
//! the rows returned must be right. Cost-based index selection is asserted where
//! it is stable, but a slower plan is never a wrong plan.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{CompareOp, CountQuery, ExistsQuery, Filter, FindQuery, OrderKey, Strategy};
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
        .with_field(FieldDef {
            name: "category".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: true,
            indexed: true,
        })
        .with_field(FieldDef::new("views", FieldType::Int))
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("metadata", FieldType::Document))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
        .with_index(IndexDef {
            path: "metadata.source".into(),
            kind: IndexKind::DocumentPath,
        })
}

fn open() -> (tempfile::TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    (dir, engine)
}

fn doc(id: &str, status: &str, category: &str, views: i64, source: &str) -> Document {
    let mut m = Document::new();
    m.insert("id".into(), Value::Text(id.into()));
    m.insert("status".into(), Value::Text(status.into()));
    m.insert("category".into(), Value::Text(category.into()));
    m.insert("views".into(), Value::Int(views));
    m.insert("body".into(), Value::Text("alpha beta gamma".into()));
    let mut meta = Document::new();
    meta.insert("source".into(), Value::Text(source.into()));
    m.insert("metadata".into(), Value::Object(meta));
    m
}

fn eq(field: &str, value: &str) -> Filter {
    Filter::Compare {
        field: field.into(),
        op: CompareOp::Eq,
        value: Value::Text(value.into()),
    }
}

/// Seed `n` docs where `status` has two values (low cardinality) and `category`
/// is unique per row (high cardinality, more selective).
fn seed(engine: &Engine, n: usize) {
    for i in 0..n {
        let status = if i % 2 == 0 { "even" } else { "odd" };
        engine
            .insert(
                "Doc",
                doc(
                    &format!("d{i}"),
                    status,
                    &format!("cat{i}"),
                    i as i64,
                    if i == 0 { "import" } else { "manual" },
                ),
            )
            .unwrap();
    }
}

#[test]
fn planner_selects_more_selective_secondary_index() {
    let (_d, engine) = open();
    seed(&engine, 40);
    engine.analyze().unwrap();
    // An AND on a low-cardinality and a high-cardinality indexed field: the
    // planner should seed selection from the more selective `category`.
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::And {
        filters: vec![eq("status", "even"), eq("category", "cat10")],
    });
    let plan = engine.explain(&q).unwrap();
    assert_eq!(plan.strategy, Strategy::IndexLookup);
    assert_eq!(plan.used_index.as_deref(), Some("category"));
    // And the result is correct regardless of the chosen index.
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields.get("id"), Some(&Value::Text("d10".into())));
}

#[test]
fn planner_selects_doc_path_over_scan() {
    let (_d, engine) = open();
    seed(&engine, 30);
    engine.analyze().unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("metadata.source", "import"));
    let plan = engine.explain(&q).unwrap();
    assert_eq!(plan.strategy, Strategy::IndexLookup);
    assert_eq!(plan.used_index.as_deref(), Some("metadata.source"));
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn planner_selects_full_text_over_scan() {
    let (_d, engine) = open();
    seed(&engine, 10);
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::ContainsText {
        field: "body".into(),
        query: "alpha".into(),
    });
    let plan = engine.explain(&q).unwrap();
    assert_eq!(plan.strategy, Strategy::FullTextScan);
    assert_eq!(engine.find(&q).unwrap().len(), 10);
}

#[test]
fn planner_does_not_use_index_for_unsupported_operator() {
    let (_d, engine) = open();
    seed(&engine, 20);
    engine.analyze().unwrap();
    // A range predicate cannot be served by an equality index: fall back to scan
    // but still return the correct rows.
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::Compare {
        field: "views".into(),
        op: CompareOp::Gt,
        value: Value::Int(15),
    });
    let plan = engine.explain(&q).unwrap();
    assert_eq!(plan.strategy, Strategy::FullScan);
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 4); // views 16,17,18,19
}

#[test]
fn planner_uses_latest_stats_after_analyze() {
    let (_d, engine) = open();
    seed(&engine, 50);
    engine.analyze().unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("category", "cat3"));
    let plan = engine.explain(&q).unwrap();
    assert_eq!(plan.strategy, Strategy::IndexLookup);
    assert_eq!(plan.used_index.as_deref(), Some("category"));
}

#[test]
fn planner_handles_stale_stats_without_wrong_results() {
    let (_d, engine) = open();
    seed(&engine, 10);
    engine.analyze().unwrap();
    // Insert more rows *after* analyze so the persisted stats are stale.
    for i in 10..30 {
        engine
            .insert(
                "Doc",
                doc(
                    &format!("d{i}"),
                    "even",
                    &format!("cat{i}"),
                    i as i64,
                    "manual",
                ),
            )
            .unwrap();
    }
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("status", "even"));
    // Stale stats may change the cost choice, never the result.
    let rows = engine.find(&q).unwrap();
    let expected = engine
        .find(&FindQuery::new("Doc"))
        .unwrap()
        .iter()
        .filter(|r| r.fields.get("status") == Some(&Value::Text("even".into())))
        .count();
    assert_eq!(rows.len(), expected);
}

#[test]
fn planner_stable_plan_for_same_stats() {
    let (_d, engine) = open();
    seed(&engine, 30);
    engine.analyze().unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("category", "cat5"));
    let a = engine.explain(&q).unwrap();
    let b = engine.explain(&q).unwrap();
    assert_eq!(a.strategy, b.strategy);
    assert_eq!(a.used_index, b.used_index);
    assert_eq!(a.estimated_rows, b.estimated_rows);
    assert_eq!(a.estimated_cost, b.estimated_cost);
}

#[test]
fn planner_explain_reports_cost_and_reason() {
    let (_d, engine) = open();
    seed(&engine, 20);
    engine.analyze().unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("category", "cat1"));
    let plan = engine.explain_analyze(&q).unwrap();
    assert!(plan.estimated_cost >= 0.0);
    let a = plan.analysis.expect("analysis present");
    let reason = a.selected_index_reason.expect("selection reason present");
    assert!(reason.contains("category"), "{reason}");
}

#[test]
fn planner_count_uses_index_when_possible() {
    let (_d, engine) = open();
    seed(&engine, 20);
    engine.analyze().unwrap();
    let q = CountQuery {
        collection: "Doc".into(),
        filter: Some(eq("status", "even")),
    };
    assert_eq!(engine.count(&q).unwrap(), 10);
    // The planner can serve the same equality predicate from an index.
    let mut find = FindQuery::new("Doc");
    find.filter = Some(eq("status", "even"));
    assert_eq!(
        engine.explain(&find).unwrap().strategy,
        Strategy::IndexLookup
    );
}

#[test]
fn planner_exists_short_circuits_when_possible() {
    let (_d, engine) = open();
    seed(&engine, 20);
    let present = ExistsQuery {
        collection: "Doc".into(),
        filter: Some(eq("category", "cat7")),
    };
    let absent = ExistsQuery {
        collection: "Doc".into(),
        filter: Some(eq("category", "nope")),
    };
    assert!(engine.exists(&present).unwrap());
    assert!(!engine.exists(&absent).unwrap());
}

#[test]
fn planner_sort_limit_order_correctness() {
    let (_d, engine) = open();
    seed(&engine, 20);
    let mut q = FindQuery::new("Doc");
    q.order_by = vec![OrderKey {
        field: "views".into(),
        desc: true,
    }];
    q.limit = Some(3);
    let rows = engine.find(&q).unwrap();
    let views: Vec<i64> = rows
        .iter()
        .map(|r| match r.fields.get("views") {
            Some(Value::Int(v)) => *v,
            _ => -1,
        })
        .collect();
    assert_eq!(views, vec![19, 18, 17]);
}

#[test]
fn planner_projection_does_not_affect_filter_correctness() {
    let (_d, engine) = open();
    seed(&engine, 20);
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("status", "odd"));
    q.projection = Some(vec!["id".into()]);
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 10);
    // Projection keeps only `id`; the filter still matched the right rows.
    for r in &rows {
        assert!(r.fields.contains_key("id"));
        assert!(!r.fields.contains_key("views"));
    }
}

#[test]
fn planner_mvcc_snapshot_filter_correctness() {
    let (_d, engine) = open();
    seed(&engine, 5);
    let txn = engine.begin();
    // A concurrent committed insert is invisible to the pinned snapshot.
    engine
        .insert("Doc", doc("late", "even", "catX", 99, "manual"))
        .unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq("status", "even"));
    // The snapshot sees the 3 original even rows (d0,d2,d4), not the late insert.
    let txn_rows = engine.txn_find(&txn, &q).unwrap();
    assert_eq!(txn_rows.len(), 3);
    // A non-transactional reader sees the late insert too.
    assert_eq!(engine.find(&q).unwrap().len(), 4);
    engine.rollback(txn);
}
