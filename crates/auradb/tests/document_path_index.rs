//! Document-path index tests: equality acceleration over nested document
//! values, EXPLAIN reporting, update/delete maintenance, restart persistence,
//! and schema validation of index paths.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{CompareOp, Filter, FindQuery, Mutation, Strategy};
use auradb::Engine;

fn schema() -> CollectionSchema {
    CollectionSchema::new("User")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("profile", FieldType::Document))
        .with_index(IndexDef {
            path: "profile.company".into(),
            kind: IndexKind::DocumentPath,
        })
}

fn user(id: &str, company: Option<&str>) -> Document {
    let mut profile = Document::new();
    if let Some(c) = company {
        profile.insert("company".into(), Value::Text(c.into()));
    }
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(id.into()));
    f.insert("profile".into(), Value::Object(profile));
    f
}

fn company_eq(value: &str) -> FindQuery {
    let mut q = FindQuery::new("User");
    q.filter = Some(Filter::Compare {
        field: "profile.company".into(),
        op: CompareOp::Eq,
        value: Value::Text(value.into()),
    });
    q
}

fn seed(engine: &Engine) {
    engine.create_schema(schema()).unwrap();
    engine.insert("User", user("u1", Some("acme"))).unwrap();
    engine.insert("User", user("u2", Some("globex"))).unwrap();
    engine.insert("User", user("u3", Some("acme"))).unwrap();
    engine.insert("User", user("u4", None)).unwrap();
}

#[test]
fn equality_filter_uses_document_path_index() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);

    assert_eq!(engine.find(&company_eq("acme")).unwrap().len(), 2);
    assert_eq!(engine.find(&company_eq("globex")).unwrap().len(), 1);

    let plan = engine.explain(&company_eq("acme")).unwrap();
    assert_eq!(plan.strategy, Strategy::IndexLookup);
    assert_eq!(plan.used_index.as_deref(), Some("profile.company"));
}

#[test]
fn missing_path_is_not_matched() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    // u4 has no profile.company; it is excluded from any company equality match.
    assert_eq!(engine.find(&company_eq("acme")).unwrap().len(), 2);
    assert!(engine.find(&company_eq("nonexistent")).unwrap().is_empty());
}

#[test]
fn update_moves_indexed_value() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);

    // Re-point u1's company from acme to initech (replace the profile document).
    let mut set = Document::new();
    let mut profile = Document::new();
    profile.insert("company".into(), Value::Text("initech".into()));
    set.insert("profile".into(), Value::Object(profile));
    engine
        .apply_mutation(Mutation::Update {
            collection: "User".into(),
            filter: Some(Filter::Compare {
                field: "id".into(),
                op: CompareOp::Eq,
                value: Value::Text("u1".into()),
            }),
            set,
        })
        .unwrap();

    assert_eq!(engine.find(&company_eq("acme")).unwrap().len(), 1); // only u3
    assert_eq!(engine.find(&company_eq("initech")).unwrap().len(), 1);
}

#[test]
fn delete_removes_indexed_value() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    engine
        .apply_mutation(Mutation::Delete {
            collection: "User".into(),
            filter: Some(Filter::Compare {
                field: "id".into(),
                op: CompareOp::Eq,
                value: Value::Text("u3".into()),
            }),
        })
        .unwrap();
    assert_eq!(engine.find(&company_eq("acme")).unwrap().len(), 1);
}

#[test]
fn document_path_index_persists_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seed(&engine);
        engine.checkpoint().unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    assert_eq!(engine.index_load_report().rebuilt, 0);
    assert_eq!(engine.find(&company_eq("acme")).unwrap().len(), 2);
    let plan = engine.explain(&company_eq("acme")).unwrap();
    assert_eq!(plan.used_index.as_deref(), Some("profile.company"));
}

#[test]
fn invalid_index_path_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    // Index path root must be a declared field.
    let bad = CollectionSchema::new("Bad")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_index(IndexDef {
            path: "nonexistent.path".into(),
            kind: IndexKind::DocumentPath,
        });
    assert!(engine.create_schema(bad).is_err());
}
