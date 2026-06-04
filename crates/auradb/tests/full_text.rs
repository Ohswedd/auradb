//! Full-text search tests: tokenization, case folding, punctuation, multi-term
//! AND matching, term-frequency ranking, index maintenance on update/delete,
//! restart persistence, EXPLAIN reporting, and the honest no-index scan path.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{Filter, FindQuery, Strategy};
use auradb::Engine;

fn schema(with_index: bool) -> CollectionSchema {
    let s = CollectionSchema::new("Doc")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("body", FieldType::String));
    if with_index {
        s.with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
    } else {
        s
    }
}

fn doc(id: &str, body: &str) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(id.into()));
    f.insert("body".into(), Value::Text(body.into()));
    f
}

fn contains(query: &str) -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::ContainsText {
        field: "body".into(),
        query: query.into(),
    });
    q
}

fn row_id(row: &auradb::query::Row) -> String {
    match row.fields.get("id") {
        Some(Value::Text(s)) => s.clone(),
        _ => String::new(),
    }
}

#[test]
fn tokenization_case_and_punctuation() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema(true)).unwrap();
    engine
        .insert("Doc", doc("d1", "The Quick, Brown Fox!"))
        .unwrap();
    engine
        .insert("Doc", doc("d2", "a quick refund request"))
        .unwrap();

    // Case-insensitive single term.
    assert_eq!(engine.find(&contains("QUICK")).unwrap().len(), 2);
    // Punctuation is a token boundary; multi-term AND.
    assert_eq!(engine.find(&contains("brown fox")).unwrap().len(), 1);
    // Missing term excludes the record.
    assert!(engine.find(&contains("quick cat")).unwrap().is_empty());
    // Empty query matches nothing.
    assert!(engine.find(&contains("   ")).unwrap().is_empty());
}

#[test]
fn ranking_by_term_frequency() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema(true)).unwrap();
    engine.insert("Doc", doc("low", "alpha beta")).unwrap();
    engine
        .insert("Doc", doc("high", "alpha alpha alpha beta"))
        .unwrap();

    let rows = engine.find(&contains("alpha")).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        row_id(&rows[0]),
        "high",
        "higher term frequency ranks first"
    );
    assert!(rows[0].score.unwrap() > rows[1].score.unwrap());
}

#[test]
fn explain_reports_full_text_index() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema(true)).unwrap();
    engine.insert("Doc", doc("d1", "hello world")).unwrap();
    let plan = engine.explain(&contains("hello")).unwrap();
    assert_eq!(plan.strategy, Strategy::FullTextScan);
    assert_eq!(plan.used_index.as_deref(), Some("body"));
}

#[test]
fn update_and_delete_maintain_index() {
    use auradb::query::{CompareOp, Mutation};
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema(true)).unwrap();
    engine.insert("Doc", doc("d1", "original text")).unwrap();
    engine.insert("Doc", doc("d2", "keep this text")).unwrap();

    // Update d1's body: the old term "original" is removed, "rewritten" added.
    let mut set = Document::new();
    set.insert("body".into(), Value::Text("rewritten text".into()));
    engine
        .apply_mutation(Mutation::Update {
            collection: "Doc".into(),
            filter: Some(Filter::Compare {
                field: "id".into(),
                op: CompareOp::Eq,
                value: Value::Text("d1".into()),
            }),
            set,
        })
        .unwrap();
    assert!(engine.find(&contains("original")).unwrap().is_empty());
    assert_eq!(engine.find(&contains("rewritten")).unwrap().len(), 1);

    // Delete d2: its terms disappear.
    engine
        .apply_mutation(Mutation::Delete {
            collection: "Doc".into(),
            filter: Some(Filter::Compare {
                field: "id".into(),
                op: CompareOp::Eq,
                value: Value::Text("d2".into()),
            }),
        })
        .unwrap();
    assert!(engine.find(&contains("keep")).unwrap().is_empty());
    assert_eq!(engine.find(&contains("text")).unwrap().len(), 1);
}

#[test]
fn full_text_index_persists_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        engine.create_schema(schema(true)).unwrap();
        engine
            .insert("Doc", doc("d1", "persisted inverted index"))
            .unwrap();
        engine.checkpoint().unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(engine.index_load_report().rebuilt, 0);
    assert_eq!(engine.find(&contains("inverted")).unwrap().len(), 1);
    assert_eq!(
        engine.explain(&contains("inverted")).unwrap().strategy,
        Strategy::FullTextScan
    );
}

#[test]
fn text_search_without_index_falls_back_to_scan() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema(false)).unwrap(); // no full-text index
    engine.insert("Doc", doc("d1", "scan only text")).unwrap();
    engine.insert("Doc", doc("d2", "another body")).unwrap();

    // Results are still correct via a tokenized scan.
    assert_eq!(engine.find(&contains("scan")).unwrap().len(), 1);
    // EXPLAIN honestly reports a full scan, not a full-text index.
    let plan = engine.explain(&contains("scan")).unwrap();
    assert_eq!(plan.strategy, Strategy::FullScan);
    assert_eq!(plan.used_index, None);
}
