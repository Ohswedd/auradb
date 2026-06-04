//! Chaos restart testing: drive a deterministic, seeded stream of writes,
//! updates, deletes, and transactions against the engine, forcing a restart
//! (drop and reopen the engine from disk) at fixed intervals, and compare the
//! recovered state against an in-memory reference model after every restart.
//!
//! The default test is CI-safe: bounded operations, deterministic seeds, and no
//! timing-based sleeps. A heavier stress run is available behind `--ignored`.

use std::collections::BTreeMap;
use std::path::Path;

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{CompareOp, Filter, FindQuery, Mutation, VectorSearch};
use auradb::Engine;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const DIM: usize = 4;

fn schema() -> CollectionSchema {
    CollectionSchema::new("Item")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef {
            name: "name".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: false,
            indexed: true,
        })
        .with_field(FieldDef::new("tags", FieldType::Document))
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("vec", FieldType::Vector { dim: DIM }))
        .with_index(IndexDef {
            path: "tags.group".into(),
            kind: IndexKind::DocumentPath,
        })
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

/// Build a deterministic record for `id` at logical version `gen`.
fn record(id: usize, generation: u64) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(format!("item{id}")));
    f.insert("name".into(), Value::Text(format!("name-{}", id % 8)));
    let mut tags = Document::new();
    tags.insert("group".into(), Value::Text(format!("g{}", id % 5)));
    tags.insert("gen".into(), Value::Int(generation as i64));
    f.insert("tags".into(), Value::Object(tags));
    f.insert(
        "body".into(),
        Value::Text(format!("alpha bravo item {id} generation {generation}")),
    );
    let v: Vec<f32> = (0..DIM).map(|j| ((id + j) % 11) as f32).collect();
    f.insert("vec".into(), Value::Vector(v));
    f
}

/// Verify that the reopened engine exactly matches the reference model and that
/// every index path serves consistent results.
fn verify(engine: &Engine, model: &BTreeMap<String, Document>) {
    let rows = engine.find(&FindQuery::new("Item")).unwrap();
    assert_eq!(
        rows.len(),
        model.len(),
        "live record count matches the model"
    );

    let live: BTreeMap<String, Document> = rows
        .into_iter()
        .map(|r| {
            let id = match r.fields.get("id") {
                Some(Value::Text(t)) => t.clone(),
                _ => panic!("record without id"),
            };
            (id, r.fields)
        })
        .collect();
    for (id, fields) in model {
        let got = live.get(id).unwrap_or_else(|| panic!("missing {id}"));
        assert_eq!(got, fields, "record {id} matches the model after restart");
    }

    // Consistency check: indexes agree with stored records.
    engine.check_consistency().unwrap();

    // Secondary index lookup agrees with the model.
    for name_bucket in 0..8 {
        let name = format!("name-{name_bucket}");
        let mut q = FindQuery::new("Item");
        q.filter = Some(Filter::Compare {
            field: "name".into(),
            op: CompareOp::Eq,
            value: Value::Text(name.clone()),
        });
        let got: usize = engine.find(&q).unwrap().len();
        let expected = model
            .values()
            .filter(|f| matches!(f.get("name"), Some(Value::Text(t)) if *t == name))
            .count();
        assert_eq!(got, expected, "secondary index lookup for {name}");
    }

    // Document-path index lookup agrees with the model.
    for g in 0..5 {
        let group = format!("g{g}");
        let mut q = FindQuery::new("Item");
        q.filter = Some(Filter::Compare {
            field: "tags.group".into(),
            op: CompareOp::Eq,
            value: Value::Text(group.clone()),
        });
        let got = engine.find(&q).unwrap().len();
        let expected = model
            .values()
            .filter(|f| {
                matches!(f.get("tags"), Some(Value::Object(o))
                    if matches!(o.get("group"), Some(Value::Text(t)) if *t == group))
            })
            .count();
        assert_eq!(got, expected, "document-path lookup for {group}");
    }

    // Full-text lookup: every live record contains the shared tokens.
    let mut q = FindQuery::new("Item");
    q.filter = Some(Filter::ContainsText {
        field: "body".into(),
        query: "alpha bravo".into(),
    });
    assert_eq!(
        engine.find(&q).unwrap().len(),
        model.len(),
        "full-text scan"
    );

    // Vector search returns up to k scored results.
    if !model.is_empty() {
        let mut q = FindQuery::new("Item");
        q.vector = Some(VectorSearch {
            field: "vec".into(),
            query: vec![1.0; DIM],
            k: 5,
            metric: "cosine".into(),
        });
        let near = engine.find(&q).unwrap();
        assert!(near.len() <= 5);
        assert!(near.iter().all(|r| r.score.is_some()));
    }
}

/// Run the chaos loop with a given seed, op budget, and restart interval.
fn run_chaos(seed: u64, ops: usize, restart_every: usize, id_space: usize) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data");
    let mut model: BTreeMap<String, Document> = BTreeMap::new();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut generation: u64 = 0;

    let mut engine = Engine::open(&dir).unwrap();
    engine.create_schema(schema()).unwrap();

    for step in 1..=ops {
        let roll: u8 = rng.gen_range(0..10);
        if roll < 6 {
            // Upsert (insert or replace).
            let id = rng.gen_range(0..id_space);
            generation += 1;
            let fields = record(id, generation);
            engine
                .apply_mutation(Mutation::Upsert {
                    collection: "Item".into(),
                    fields: fields.clone(),
                })
                .unwrap();
            model.insert(format!("item{id}"), fields);
        } else if roll < 8 {
            // Delete by primary key.
            let id = rng.gen_range(0..id_space);
            engine
                .apply_mutation(Mutation::Delete {
                    collection: "Item".into(),
                    filter: Some(Filter::Compare {
                        field: "id".into(),
                        op: CompareOp::Eq,
                        value: Value::Text(format!("item{id}")),
                    }),
                })
                .unwrap();
            model.remove(&format!("item{id}"));
        } else {
            // Transaction staging two upserts, then commit or roll back.
            let mut txn = engine.begin();
            let mut staged = Vec::new();
            for _ in 0..2 {
                let id = rng.gen_range(0..id_space);
                generation += 1;
                let fields = record(id, generation);
                engine
                    .stage(
                        &mut txn,
                        Mutation::Upsert {
                            collection: "Item".into(),
                            fields: fields.clone(),
                        },
                    )
                    .unwrap();
                staged.push((format!("item{id}"), fields));
            }
            if rng.gen_bool(0.7) {
                engine.commit(txn).unwrap();
                for (id, fields) in staged {
                    model.insert(id, fields);
                }
            } else {
                engine.rollback(txn);
                // staged effects discarded; model unchanged.
            }
        }

        // Force a restart at deterministic intervals: drop the engine and reopen
        // from disk, then verify the recovered state.
        if step % restart_every == 0 {
            drop(engine);
            engine = Engine::open(&dir).unwrap();
            verify(&engine, &model);
        }
    }

    // Final verification and a dump/restore round trip after the chaos run.
    verify(&engine, &model);

    let dest = tempfile::tempdir().unwrap();
    let restored = Engine::open(dest.path()).unwrap();
    for s in engine.list_schemas() {
        restored.create_schema(s.clone()).unwrap();
        for row in engine.find(&FindQuery::new(&s.name)).unwrap() {
            restored
                .apply_mutation(Mutation::Upsert {
                    collection: s.name.clone(),
                    fields: row.fields,
                })
                .unwrap();
        }
    }
    verify(&restored, &model);
}

#[test]
fn chaos_restart_preserves_committed_state() {
    // A few independent seeds keep the run deterministic and broad.
    for seed in [1u64, 7, 42] {
        run_chaos(seed, 240, 30, 50);
    }
}

#[test]
#[ignore = "heavier local stress run; enable with --ignored"]
fn chaos_restart_stress() {
    let _ = Path::new("");
    run_chaos(2024, 4000, 100, 200);
}
