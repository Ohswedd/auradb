//! Embed the AuraDB engine directly (no network) and run a small workflow:
//! schema, insert, vector search, relationship include, and a transaction.
//!
//! Run with: `cargo run -p auradb --example embedded`

use auradb::core::{
    Cardinality, CollectionSchema, Document, FieldDef, FieldType, OnDelete, Relationship, Value,
};
use auradb::query::{FindQuery, Mutation, VectorSearch};
use auradb::Engine;

fn pk(name: &str) -> FieldDef {
    FieldDef {
        name: name.into(),
        field_type: FieldType::Uuid,
        primary_key: true,
        unique: true,
        nullable: false,
        indexed: false,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let engine = Engine::open(dir.path())?;

    engine.create_schema(CollectionSchema::new("User").with_field(pk("id")))?;
    engine.create_schema(
        CollectionSchema::new("Doc")
            .with_field(pk("id"))
            .with_field(FieldDef::new("title", FieldType::String))
            .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
            .with_relationship(Relationship {
                name: "owner".into(),
                target: "User".into(),
                cardinality: Cardinality::ToOne,
                on_delete: OnDelete::Restrict,
            }),
    )?;

    let mut user = Document::new();
    user.insert("id".into(), Value::Text("u1".into()));
    engine.insert("User", user)?;

    for (id, title, vec) in [
        ("d1", "Hiking jacket", vec![1.0, 0.0, 0.0]),
        ("d2", "Rain boots", vec![0.0, 1.0, 0.0]),
        ("d3", "Waterproof shell", vec![0.9, 0.1, 0.0]),
    ] {
        let mut d = Document::new();
        d.insert("id".into(), Value::Text(id.into()));
        d.insert("title".into(), Value::Text(title.into()));
        d.insert("embedding".into(), Value::Vector(vec));
        d.insert("owner".into(), Value::Text("u1".into()));
        engine.insert("Doc", d)?;
    }

    // Vector search with relationship hydration.
    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![1.0, 0.0, 0.0],
        k: 2,
        metric: "cosine".into(),
    });
    q.includes = vec!["owner".into()];
    println!("Nearest documents to [1,0,0]:");
    for row in engine.find(&q)? {
        let title = row
            .fields
            .get("title")
            .and_then(Value::as_text)
            .unwrap_or("");
        println!(
            "  {title}  score={:.3}  owners={}",
            row.score.unwrap_or(0.0),
            row.includes["owner"].len()
        );
    }

    // A transaction.
    let mut txn = engine.begin();
    let mut d = Document::new();
    d.insert("id".into(), Value::Text("d4".into()));
    d.insert("title".into(), Value::Text("Gloves".into()));
    d.insert("embedding".into(), Value::Vector(vec![0.5, 0.5, 0.0]));
    d.insert("owner".into(), Value::Text("u1".into()));
    engine.stage(
        &mut txn,
        Mutation::Insert {
            collection: "Doc".into(),
            fields: d,
        },
    )?;
    engine.commit(txn)?;

    println!(
        "Total documents: {}",
        engine.find(&FindQuery::new("Doc"))?.len()
    );
    Ok(())
}
