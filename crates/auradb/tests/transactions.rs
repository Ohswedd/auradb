//! Transaction-scoped read tests.
//!
//! These verify that reads issued with a transaction execute against the
//! *transaction view*: committed state overlaid with the transaction's own
//! staged writes and deletes. A transaction sees its staged inserts and
//! updates, does not see its staged deletes, and these effects are invisible to
//! non-transactional readers until commit. Every read surface is covered: find,
//! filter, count, exists, vector nearest, document-path filters, full-text,
//! and cursor-style planning/materialization.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{
    CompareOp, CountQuery, ExistsQuery, Filter, FindQuery, Mutation, VectorSearch,
};
use auradb::{Engine, Transaction};

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
        .with_field(FieldDef::new("title", FieldType::String))
        .with_field(FieldDef::new("views", FieldType::Int))
        .with_field(FieldDef::new("metadata", FieldType::Document))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
        .with_field(FieldDef::new("body", FieldType::String))
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

fn doc(id: &str) -> Document {
    let mut m = Document::new();
    m.insert("id".into(), Value::Text(id.into()));
    m.insert("status".into(), Value::Text("published".into()));
    m.insert("title".into(), Value::Text(id.into()));
    m.insert("views".into(), Value::Int(1));
    m.insert("body".into(), Value::Text("alpha beta".into()));
    let mut meta = Document::new();
    meta.insert("source".into(), Value::Text("import".into()));
    m.insert("metadata".into(), Value::Object(meta));
    m
}

fn stage_insert(engine: &Engine, txn: &mut Transaction, fields: Document) {
    engine
        .stage(
            txn,
            Mutation::Insert {
                collection: "Doc".into(),
                fields,
            },
        )
        .unwrap();
}

fn eq_filter(field: &str, value: Value) -> Filter {
    Filter::Compare {
        field: field.into(),
        op: CompareOp::Eq,
        value,
    }
}

#[test]
fn transactional_find_sees_staged_insert() {
    let (_dir, engine) = open();
    let mut txn = engine.begin();
    stage_insert(&engine, &mut txn, doc("d1"));

    // The transaction sees its own staged insert.
    let rows = engine.txn_find(&txn, &FindQuery::new("Doc")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields.get("id"), Some(&Value::Text("d1".into())));

    // A non-transactional reader does not.
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 0);
}

#[test]
fn transactional_filter_sees_staged_insert() {
    let (_dir, engine) = open();
    let mut txn = engine.begin();
    stage_insert(&engine, &mut txn, doc("d1"));

    // Equality filter on the indexed `status` field must hit the staged record,
    // proving the overlay index reflects staged writes.
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq_filter("status", Value::Text("published".into())));
    let rows = engine.txn_find(&txn, &q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields.get("id"), Some(&Value::Text("d1".into())));
}

#[test]
fn transactional_count_sees_staged_insert() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("committed")).unwrap();
    let mut txn = engine.begin();
    stage_insert(&engine, &mut txn, doc("d1"));

    let q = CountQuery {
        collection: "Doc".into(),
        filter: None,
    };
    // Committed + one staged.
    assert_eq!(engine.txn_count(&txn, &q).unwrap(), 2);
    // Non-transactional count sees only the committed record.
    assert_eq!(engine.count(&q).unwrap(), 1);
}

#[test]
fn transactional_exists_sees_staged_insert() {
    let (_dir, engine) = open();
    let mut txn = engine.begin();
    stage_insert(&engine, &mut txn, doc("d1"));

    let q = ExistsQuery {
        collection: "Doc".into(),
        filter: Some(eq_filter("id", Value::Text("d1".into()))),
    };
    assert!(engine.txn_exists(&txn, &q).unwrap());
    assert!(!engine.exists(&q).unwrap());
}

#[test]
fn transactional_read_hides_staged_delete() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1")).unwrap();

    let mut txn = engine.begin();
    engine
        .stage(
            &mut txn,
            Mutation::Delete {
                collection: "Doc".into(),
                filter: Some(eq_filter("id", Value::Text("d1".into()))),
            },
        )
        .unwrap();

    // The transaction must not see its own staged delete.
    assert_eq!(
        engine.txn_find(&txn, &FindQuery::new("Doc")).unwrap().len(),
        0
    );
    assert_eq!(
        engine
            .txn_count(
                &txn,
                &CountQuery {
                    collection: "Doc".into(),
                    filter: None
                }
            )
            .unwrap(),
        0
    );
    // But it is still committed for everyone else until the txn commits.
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 1);
}

#[test]
fn transactional_update_visible_before_commit() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1")).unwrap();

    let mut txn = engine.begin();
    let mut set = Document::new();
    set.insert("title".into(), Value::Text("from-txn".into()));
    engine
        .stage(
            &mut txn,
            Mutation::Update {
                collection: "Doc".into(),
                filter: Some(eq_filter("id", Value::Text("d1".into()))),
                set,
            },
        )
        .unwrap();

    // The transaction sees the updated value before commit.
    let rows = engine.txn_find(&txn, &FindQuery::new("Doc")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("title"),
        Some(&Value::Text("from-txn".into()))
    );
    // Non-transactional readers still see the committed value.
    let rows = engine.find(&FindQuery::new("Doc")).unwrap();
    assert_eq!(rows[0].fields.get("title"), Some(&Value::Text("d1".into())));
}

#[test]
fn transactional_vector_query_sees_staged_vector() {
    let (_dir, engine) = open();
    let mut txn = engine.begin();
    let mut fields = doc("d1");
    fields.insert("embedding".into(), Value::Vector(vec![1.0, 0.0, 0.0]));
    stage_insert(&engine, &mut txn, fields);

    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![1.0, 0.0, 0.0],
        k: 3,
        metric: "cosine".into(),
    });
    // Vector nearest runs against the overlay index, so the staged vector is a
    // candidate.
    let rows = engine.txn_find(&txn, &q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields.get("id"), Some(&Value::Text("d1".into())));
    // The committed vector index is still empty.
    assert_eq!(engine.find(&q).unwrap().len(), 0);
}

#[test]
fn transactional_document_filter_sees_staged_document_update() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1")).unwrap();

    let mut txn = engine.begin();
    let mut set = Document::new();
    let mut meta = Document::new();
    meta.insert("source".into(), Value::Text("staged".into()));
    set.insert("metadata".into(), Value::Object(meta));
    engine
        .stage(
            &mut txn,
            Mutation::Update {
                collection: "Doc".into(),
                filter: Some(eq_filter("id", Value::Text("d1".into()))),
                set,
            },
        )
        .unwrap();

    // Nested document-path filter sees the staged document update.
    let mut q = FindQuery::new("Doc");
    q.filter = Some(eq_filter("metadata.source", Value::Text("staged".into())));
    assert_eq!(engine.txn_find(&txn, &q).unwrap().len(), 1);
    // The old committed document value is shadowed inside the transaction.
    let mut q_old = FindQuery::new("Doc");
    q_old.filter = Some(eq_filter("metadata.source", Value::Text("import".into())));
    assert_eq!(engine.txn_find(&txn, &q_old).unwrap().len(), 0);
    // Non-transactional readers still match the committed value.
    assert_eq!(engine.find(&q_old).unwrap().len(), 1);
}

#[test]
fn transactional_full_text_sees_staged_text_update() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("d1")).unwrap(); // body = "alpha beta"

    let mut txn = engine.begin();
    let mut set = Document::new();
    set.insert("body".into(), Value::Text("gamma delta".into()));
    engine
        .stage(
            &mut txn,
            Mutation::Update {
                collection: "Doc".into(),
                filter: Some(eq_filter("id", Value::Text("d1".into()))),
                set,
            },
        )
        .unwrap();

    let contains = |term: &str| {
        let mut q = FindQuery::new("Doc");
        q.filter = Some(Filter::ContainsText {
            field: "body".into(),
            query: term.into(),
        });
        q
    };

    // The full-text overlay index reflects the staged text update.
    assert_eq!(engine.txn_find(&txn, &contains("gamma")).unwrap().len(), 1);
    // The previous committed term no longer matches inside the transaction.
    assert_eq!(engine.txn_find(&txn, &contains("alpha")).unwrap().len(), 0);
    // The committed index is untouched until commit.
    assert_eq!(engine.find(&contains("alpha")).unwrap().len(), 1);
    assert_eq!(engine.find(&contains("gamma")).unwrap().len(), 0);
}

#[test]
fn transactional_cursor_uses_transaction_view() {
    let (_dir, engine) = open();
    engine.insert("Doc", doc("committed")).unwrap();

    let mut txn = engine.begin();
    stage_insert(&engine, &mut txn, doc("staged-1"));
    stage_insert(&engine, &mut txn, doc("staged-2"));

    // Cursor creation plans against the transaction view: the ordered id list
    // includes staged inserts...
    let planned = engine.txn_plan_find(&txn, &FindQuery::new("Doc")).unwrap();
    assert_eq!(planned.ordered.len(), 3);

    // ...and paging (materialization) resolves staged records correctly.
    let page = engine
        .txn_materialize(&txn, &FindQuery::new("Doc"), &planned.ordered[..2])
        .unwrap();
    assert_eq!(page.len(), 2);
    let mut ids: Vec<String> = engine
        .txn_materialize(&txn, &FindQuery::new("Doc"), &planned.ordered)
        .unwrap()
        .into_iter()
        .filter_map(|r| match r.fields.get("id") {
            Some(Value::Text(s)) => Some(s.clone()),
            _ => None,
        })
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["committed", "staged-1", "staged-2"]);
}

#[test]
fn rollback_removes_transaction_view_changes() {
    let (_dir, engine) = open();
    let mut txn = engine.begin();
    stage_insert(&engine, &mut txn, doc("d1"));
    assert_eq!(
        engine.txn_find(&txn, &FindQuery::new("Doc")).unwrap().len(),
        1
    );

    engine.rollback(txn);

    // Nothing was committed, and a fresh transaction starts from clean state.
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 0);
    let txn2 = engine.begin();
    assert_eq!(
        engine
            .txn_find(&txn2, &FindQuery::new("Doc"))
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn non_transactional_reader_does_not_see_uncommitted_writes() {
    let (_dir, engine) = open();
    let mut txn = engine.begin();
    stage_insert(&engine, &mut txn, doc("d1"));

    // While the transaction is open with staged (uncommitted) writes, a
    // non-transactional reader observes none of them across every read surface.
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 0);
    assert_eq!(
        engine
            .count(&CountQuery {
                collection: "Doc".into(),
                filter: None
            })
            .unwrap(),
        0
    );
    assert!(!engine
        .exists(&ExistsQuery {
            collection: "Doc".into(),
            filter: None
        })
        .unwrap());

    // After commit they become visible.
    engine.commit(txn).unwrap();
    assert_eq!(engine.find(&FindQuery::new("Doc")).unwrap().len(), 1);
}
