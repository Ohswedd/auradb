//! End-to-end engine tests covering queries, documents, vectors, relationships,
//! transactions, recovery, and compaction.

use auradb::core::{
    Cardinality, CollectionSchema, Document, FieldDef, FieldType, OnDelete, Relationship, Value,
};
use auradb::query::{
    CompareOp, CountQuery, ExistsQuery, Filter, FindQuery, Mutation, OrderKey, VectorSearch,
};
use auradb::Engine;

fn user_schema() -> CollectionSchema {
    CollectionSchema::new("User").with_field(FieldDef {
        name: "id".into(),
        field_type: FieldType::Uuid,
        primary_key: true,
        unique: true,
        nullable: false,
        indexed: false,
    })
}

fn doc_schema() -> CollectionSchema {
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
            name: "email".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: true,
            nullable: true,
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
        .with_field(FieldDef::new("title", FieldType::String))
        .with_field(FieldDef::new("views", FieldType::Int))
        .with_field(FieldDef::new("metadata", FieldType::Document))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
        .with_relationship(Relationship {
            name: "owner".into(),
            target: "User".into(),
            cardinality: Cardinality::ToOne,
            on_delete: OnDelete::Restrict,
        })
}

fn doc(id: &str, status: &str, title: &str, views: i64, owner: &str) -> Document {
    let mut m = Document::new();
    m.insert("id".into(), Value::Text(id.into()));
    m.insert("email".into(), Value::Text(format!("{id}@x.com")));
    m.insert("status".into(), Value::Text(status.into()));
    m.insert("title".into(), Value::Text(title.into()));
    m.insert("views".into(), Value::Int(views));
    m.insert("owner".into(), Value::Text(owner.into()));
    let mut meta = Document::new();
    meta.insert("source".into(), Value::Text("import".into()));
    m.insert("metadata".into(), Value::Object(meta));
    m
}

fn open(dir: &std::path::Path) -> Engine {
    Engine::open(dir).unwrap()
}

fn seed(engine: &Engine) {
    engine.create_schema(user_schema()).unwrap();
    engine.create_schema(doc_schema()).unwrap();
    let mut u = Document::new();
    u.insert("id".into(), Value::Text("user-1".into()));
    engine.insert("User", u).unwrap();
}

#[test]
fn crud_find_and_get() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    engine
        .insert("Doc", doc("d1", "published", "Hello", 10, "user-1"))
        .unwrap();
    let rows = engine.find(&FindQuery::new("Doc")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("title"),
        Some(&Value::Text("Hello".into()))
    );
}

#[test]
fn filter_comparisons_contains_logic() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    engine
        .insert("Doc", doc("d1", "published", "Refund policy", 50, "user-1"))
        .unwrap();
    engine
        .insert("Doc", doc("d2", "draft", "Other", 5, "user-1"))
        .unwrap();

    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::And {
        filters: vec![
            Filter::Compare {
                field: "views".into(),
                op: CompareOp::Gte,
                value: Value::Int(10),
            },
            Filter::Contains {
                field: "title".into(),
                substring: "Refund".into(),
            },
        ],
    });
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields.get("id"), Some(&Value::Text("d1".into())));
}

#[test]
fn order_limit_offset_and_projection() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    for i in 0..5 {
        engine
            .insert("Doc", doc(&format!("d{i}"), "published", "t", i, "user-1"))
            .unwrap();
    }
    let mut q = FindQuery::new("Doc");
    q.order_by = vec![OrderKey {
        field: "views".into(),
        desc: true,
    }];
    q.limit = Some(2);
    q.offset = Some(1);
    q.projection = Some(vec!["views".into()]);
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 2);
    // Sorted desc by views = 4,3,2,1,0 -> offset 1 -> [3,2].
    assert_eq!(rows[0].fields.get("views"), Some(&Value::Int(3)));
    assert_eq!(rows[1].fields.get("views"), Some(&Value::Int(2)));
    // Projection keeps only "views".
    assert_eq!(rows[0].fields.len(), 1);
}

#[test]
fn count_and_exists() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    engine
        .insert("Doc", doc("d1", "published", "t", 1, "user-1"))
        .unwrap();
    engine
        .insert("Doc", doc("d2", "draft", "t", 1, "user-1"))
        .unwrap();
    let c = engine
        .count(&CountQuery {
            collection: "Doc".into(),
            filter: Some(Filter::Compare {
                field: "status".into(),
                op: CompareOp::Eq,
                value: Value::Text("published".into()),
            }),
        })
        .unwrap();
    assert_eq!(c, 1);
    assert!(engine
        .exists(&ExistsQuery {
            collection: "Doc".into(),
            filter: None
        })
        .unwrap());
}

#[test]
fn upsert_replaces_and_insert_rejects_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    engine
        .insert("Doc", doc("d1", "draft", "v1", 1, "user-1"))
        .unwrap();
    // Duplicate insert fails.
    assert!(engine
        .apply_mutation(Mutation::Insert {
            collection: "Doc".into(),
            fields: doc("d1", "draft", "v2", 1, "user-1"),
        })
        .is_err());
    // Upsert replaces.
    engine
        .apply_mutation(Mutation::Upsert {
            collection: "Doc".into(),
            fields: doc("d1", "published", "v2", 9, "user-1"),
        })
        .unwrap();
    let rows = engine.find(&FindQuery::new("Doc")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields.get("title"), Some(&Value::Text("v2".into())));
}

#[test]
fn bulk_insert_update_delete_by_filter() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    engine
        .apply_mutation(Mutation::BulkInsert {
            collection: "Doc".into(),
            records: vec![
                doc("d1", "draft", "a", 1, "user-1"),
                doc("d2", "draft", "b", 2, "user-1"),
                doc("d3", "draft", "c", 3, "user-1"),
            ],
        })
        .unwrap();
    let r = engine
        .apply_mutation(Mutation::Update {
            collection: "Doc".into(),
            filter: Some(Filter::Compare {
                field: "views".into(),
                op: CompareOp::Gte,
                value: Value::Int(2),
            }),
            set: {
                let mut s = Document::new();
                s.insert("status".into(), Value::Text("published".into()));
                s
            },
        })
        .unwrap();
    assert_eq!(r.updated, 2);
    let r = engine
        .apply_mutation(Mutation::Delete {
            collection: "Doc".into(),
            filter: Some(Filter::Compare {
                field: "status".into(),
                op: CompareOp::Eq,
                value: Value::Text("draft".into()),
            }),
        })
        .unwrap();
    assert_eq!(r.deleted, 1);
    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "Doc".into(),
                filter: None
            })
            .unwrap(),
        2
    );
}

#[test]
fn unique_violation_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    let mut a = doc("d1", "x", "a", 1, "user-1");
    a.insert("email".into(), Value::Text("dup@x.com".into()));
    let mut b = doc("d2", "x", "b", 1, "user-1");
    b.insert("email".into(), Value::Text("dup@x.com".into()));
    engine.insert("Doc", a).unwrap();
    assert!(engine.insert("Doc", b).is_err());
}

#[test]
fn document_nested_field_filter() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    engine
        .insert("Doc", doc("d1", "x", "t", 1, "user-1"))
        .unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::Compare {
        field: "metadata.source".into(),
        op: CompareOp::Eq,
        value: Value::Text("import".into()),
    });
    assert_eq!(engine.find(&q).unwrap().len(), 1);
}

#[test]
fn vector_nearest_search() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    let mk = |id: &str, v: Vec<f32>| {
        let mut d = doc(id, "x", "t", 1, "user-1");
        d.insert("embedding".into(), Value::Vector(v));
        d
    };
    engine.insert("Doc", mk("d1", vec![1.0, 0.0, 0.0])).unwrap();
    engine.insert("Doc", mk("d2", vec![0.0, 1.0, 0.0])).unwrap();
    engine.insert("Doc", mk("d3", vec![0.9, 0.1, 0.0])).unwrap();
    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![1.0, 0.0, 0.0],
        k: 2,
        metric: "cosine".into(),
    });
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].fields.get("id"), Some(&Value::Text("d1".into())));
    assert!(rows[0].score.unwrap() >= rows[1].score.unwrap());
}

#[test]
fn relationship_include_and_referential_integrity() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    // Inserting a doc referencing a missing user fails (referential integrity).
    assert!(engine
        .insert("Doc", doc("d1", "x", "t", 1, "ghost"))
        .is_err());
    engine
        .insert("Doc", doc("d1", "x", "t", 1, "user-1"))
        .unwrap();
    let mut q = FindQuery::new("Doc");
    q.includes = vec!["owner".into()];
    let rows = engine.find(&q).unwrap();
    let owners = &rows[0].includes["owner"];
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].get("id"), Some(&Value::Text("user-1".into())));

    // Deleting the referenced user is restricted.
    let del = engine.apply_mutation(Mutation::Delete {
        collection: "User".into(),
        filter: None,
    });
    assert!(del.is_err());
}

#[test]
fn explain_and_migration_estimate() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    engine
        .insert("Doc", doc("d1", "published", "t", 1, "user-1"))
        .unwrap();
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::Compare {
        field: "status".into(),
        op: CompareOp::Eq,
        value: Value::Text("published".into()),
    });
    let plan = engine.explain(&q).unwrap();
    assert_eq!(plan.used_index.as_deref(), Some("status"));

    // Migration: add an indexed field.
    let target = doc_schema().with_field(FieldDef {
        name: "category".into(),
        field_type: FieldType::String,
        primary_key: false,
        unique: false,
        nullable: true,
        indexed: true,
    });
    let est = engine.migration_estimate(&target).unwrap();
    assert!(est.exists);
    assert!(est.new_indexes.contains(&"category".to_string()));
    assert_eq!(est.records_affected, 1);
}

#[test]
fn transaction_commit_persists_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = open(dir.path());
        seed(&engine);
        let mut txn = engine.begin();
        engine
            .stage(
                &mut txn,
                Mutation::Insert {
                    collection: "Doc".into(),
                    fields: doc("d1", "x", "a", 1, "user-1"),
                },
            )
            .unwrap();
        engine
            .stage(
                &mut txn,
                Mutation::Insert {
                    collection: "Doc".into(),
                    fields: doc("d2", "x", "b", 2, "user-1"),
                },
            )
            .unwrap();
        // Not visible before commit.
        assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 0);
        engine.commit(txn).unwrap();
        assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 2);
    }
    let engine = open(dir.path());
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 2);
}

#[test]
fn transaction_rollback_discards() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    let mut txn = engine.begin();
    let result = engine
        .stage(
            &mut txn,
            Mutation::Insert {
                collection: "Doc".into(),
                fields: doc("d1", "x", "a", 1, "user-1"),
            },
        )
        .unwrap();
    // Read-your-writes within the transaction.
    let id: auradb::core::RecordId = result.ids[0].parse().unwrap();
    assert!(engine.txn_get(&txn, "Doc", id).is_some());
    engine.rollback(txn);
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 0);
}

#[test]
fn transaction_conflict_detected() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    engine
        .insert("Doc", doc("d1", "x", "a", 1, "user-1"))
        .unwrap();

    // Transaction reads/updates d1 based on its current version.
    let mut txn = engine.begin();
    engine
        .stage(
            &mut txn,
            Mutation::Update {
                collection: "Doc".into(),
                filter: Some(Filter::Compare {
                    field: "id".into(),
                    op: CompareOp::Eq,
                    value: Value::Text("d1".into()),
                }),
                set: {
                    let mut s = Document::new();
                    s.insert("title".into(), Value::Text("from-txn".into()));
                    s
                },
            },
        )
        .unwrap();

    // A concurrent auto-commit update bumps d1's version.
    engine
        .apply_mutation(Mutation::Update {
            collection: "Doc".into(),
            filter: None,
            set: {
                let mut s = Document::new();
                s.insert("title".into(), Value::Text("concurrent".into()));
                s
            },
        })
        .unwrap();

    // The transaction now conflicts.
    assert!(engine.commit(txn).is_err());
}

#[test]
fn restart_rebuilds_secondary_indexes() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = open(dir.path());
        seed(&engine);
        engine
            .insert("Doc", doc("d1", "published", "t", 1, "user-1"))
            .unwrap();
    }
    let engine = open(dir.path());
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::Compare {
        field: "status".into(),
        op: CompareOp::Eq,
        value: Value::Text("published".into()),
    });
    let plan = engine.explain(&q).unwrap();
    assert_eq!(plan.used_index.as_deref(), Some("status"));
    assert_eq!(engine.find(&q).unwrap().len(), 1);
    assert!(engine.check_consistency().unwrap() >= 1);
}

#[test]
fn compaction_then_query() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    seed(&engine);
    for i in 0..5 {
        engine
            .insert("Doc", doc(&format!("d{i}"), "x", "t", i, "user-1"))
            .unwrap();
    }
    engine
        .apply_mutation(Mutation::Delete {
            collection: "Doc".into(),
            filter: Some(Filter::Compare {
                field: "views".into(),
                op: CompareOp::Lt,
                value: Value::Int(2),
            }),
        })
        .unwrap();
    let report = engine.compact().unwrap();
    assert_eq!(report.live_records, 3 + 1); // 3 docs + 1 user
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 3);
}
