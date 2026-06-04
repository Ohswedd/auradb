//! Deterministic randomized recovery tests for the engine.
//!
//! Each seed drives a random sequence of inserts, updates, and deletes against
//! both the engine and an in-memory reference model. After a restart (with and
//! without a checkpoint, and with corrupted or missing index files) the engine
//! state must exactly match the model, and all indexes must stay consistent.
//! Seeds are fixed, so the suite is reproducible and never flaky.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{CompareOp, Filter, FindQuery, Mutation};
use auradb::Engine;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

const STATUSES: [&str; 3] = ["open", "closed", "archived"];
const TEAMS: [&str; 3] = ["red", "green", "blue"];
const WORDS: [&str; 4] = ["alpha", "beta", "gamma", "delta"];

#[derive(Clone)]
struct Rec {
    status: String,
    team: String,
    body: String,
}

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
        .with_field(FieldDef::new("profile", FieldType::Document))
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 2 }))
        .with_index(IndexDef {
            path: "profile.team".into(),
            kind: IndexKind::DocumentPath,
        })
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

fn fields(id: &str, r: &Rec) -> Document {
    let mut profile = Document::new();
    profile.insert("team".into(), Value::Text(r.team.clone()));
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(id.into()));
    f.insert("status".into(), Value::Text(r.status.clone()));
    f.insert("profile".into(), Value::Object(profile));
    f.insert("body".into(), Value::Text(r.body.clone()));
    f.insert("embedding".into(), Value::Vector(vec![1.0, 0.0]));
    f
}

fn id_filter(id: &str) -> Filter {
    Filter::Compare {
        field: "id".into(),
        op: CompareOp::Eq,
        value: Value::Text(id.into()),
    }
}

/// Apply a random op sequence to the engine and a reference model.
fn run_ops(engine: &Engine, model: &mut BTreeMap<String, Rec>, rng: &mut StdRng, ops: usize) {
    let mut next_id = 0u32;
    for _ in 0..ops {
        let choice = rng.gen_range(0..100);
        let existing: Vec<String> = model.keys().cloned().collect();
        let rand_rec = |rng: &mut StdRng| Rec {
            status: STATUSES.choose(rng).unwrap().to_string(),
            team: TEAMS.choose(rng).unwrap().to_string(),
            body: format!(
                "{} {}",
                WORDS.choose(rng).unwrap(),
                WORDS.choose(rng).unwrap()
            ),
        };
        if choice < 45 || existing.is_empty() {
            // insert
            let id = format!("r{next_id}");
            next_id += 1;
            let r = rand_rec(rng);
            engine
                .apply_mutation(Mutation::Insert {
                    collection: "Doc".into(),
                    fields: fields(&id, &r),
                })
                .unwrap();
            model.insert(id, r);
        } else if choice < 75 {
            // update an existing record (replace all mutable fields)
            let id = existing.choose(rng).unwrap().clone();
            let r = rand_rec(rng);
            let mut set = Document::new();
            set.insert("status".into(), Value::Text(r.status.clone()));
            let mut profile = Document::new();
            profile.insert("team".into(), Value::Text(r.team.clone()));
            set.insert("profile".into(), Value::Object(profile));
            set.insert("body".into(), Value::Text(r.body.clone()));
            engine
                .apply_mutation(Mutation::Update {
                    collection: "Doc".into(),
                    filter: Some(id_filter(&id)),
                    set,
                })
                .unwrap();
            model.insert(id, r);
        } else {
            // delete
            let id = existing.choose(rng).unwrap().clone();
            engine
                .apply_mutation(Mutation::Delete {
                    collection: "Doc".into(),
                    filter: Some(id_filter(&id)),
                })
                .unwrap();
            model.remove(&id);
        }
    }
}

/// Verify the engine state matches the reference model exactly.
fn verify(engine: &Engine, model: &BTreeMap<String, Rec>) {
    assert_eq!(
        engine.find(&FindQuery::new("Doc")).unwrap().len(),
        model.len()
    );

    for status in STATUSES {
        let mut q = FindQuery::new("Doc");
        q.filter = Some(Filter::Compare {
            field: "status".into(),
            op: CompareOp::Eq,
            value: Value::Text(status.into()),
        });
        let expected = model.values().filter(|r| r.status == status).count();
        assert_eq!(engine.find(&q).unwrap().len(), expected, "status {status}");
    }

    for team in TEAMS {
        let mut q = FindQuery::new("Doc");
        q.filter = Some(Filter::Compare {
            field: "profile.team".into(),
            op: CompareOp::Eq,
            value: Value::Text(team.into()),
        });
        let expected = model.values().filter(|r| r.team == team).count();
        assert_eq!(engine.find(&q).unwrap().len(), expected, "team {team}");
    }

    for word in WORDS {
        let mut q = FindQuery::new("Doc");
        q.filter = Some(Filter::ContainsText {
            field: "body".into(),
            query: word.into(),
        });
        let expected = model
            .values()
            .filter(|r| r.body.split_whitespace().any(|w| w == word))
            .count();
        assert_eq!(engine.find(&q).unwrap().len(), expected, "text {word}");
    }

    engine.check_consistency().unwrap();
}

fn any_idx_file(dir: &Path) -> Option<std::path::PathBuf> {
    fs::read_dir(dir.join("indexes"))
        .ok()?
        .flatten()
        .find_map(|e| {
            let p = e.path();
            (p.extension().map(|x| x == "idx").unwrap_or(false)).then_some(p)
        })
}

#[test]
fn random_ops_survive_checkpoint_restart() {
    for seed in 0..16u64 {
        let dir = tempfile::tempdir().unwrap();
        let mut model = BTreeMap::new();
        {
            let engine = Engine::open(dir.path()).unwrap();
            engine.create_schema(schema()).unwrap();
            let mut rng = StdRng::seed_from_u64(seed);
            run_ops(&engine, &mut model, &mut rng, 60);
            engine.checkpoint().unwrap();
        }
        let engine = Engine::open(dir.path()).unwrap();
        assert_eq!(engine.index_load_report().rebuilt, 0, "seed {seed}: loaded");
        verify(&engine, &model);
    }
}

#[test]
fn random_ops_survive_restart_without_checkpoint() {
    for seed in 100..116u64 {
        let dir = tempfile::tempdir().unwrap();
        let mut model = BTreeMap::new();
        {
            let engine = Engine::open(dir.path()).unwrap();
            engine.create_schema(schema()).unwrap();
            let mut rng = StdRng::seed_from_u64(seed);
            run_ops(&engine, &mut model, &mut rng, 60);
            // No checkpoint: indexes rebuild on next open.
        }
        let engine = Engine::open(dir.path()).unwrap();
        verify(&engine, &model);
    }
}

#[test]
fn corrupt_index_file_is_repaired_under_random_ops() {
    for seed in 200..210u64 {
        let dir = tempfile::tempdir().unwrap();
        let mut model = BTreeMap::new();
        {
            let engine = Engine::open(dir.path()).unwrap();
            engine.create_schema(schema()).unwrap();
            let mut rng = StdRng::seed_from_u64(seed);
            run_ops(&engine, &mut model, &mut rng, 50);
            engine.checkpoint().unwrap();
        }
        if let Some(idx) = any_idx_file(dir.path()) {
            let mut bytes = fs::read(&idx).unwrap();
            if !bytes.is_empty() {
                let mid = bytes.len() / 2;
                bytes[mid] ^= 0xff;
                fs::write(&idx, &bytes).unwrap();
            }
        }
        let engine = Engine::open(dir.path()).unwrap();
        assert!(engine.index_load_report().rebuilt >= 1);
        verify(&engine, &model);
    }
}

#[test]
fn corrupt_index_manifest_is_repaired() {
    let dir = tempfile::tempdir().unwrap();
    let mut model = BTreeMap::new();
    {
        let engine = Engine::open(dir.path()).unwrap();
        engine.create_schema(schema()).unwrap();
        let mut rng = StdRng::seed_from_u64(7);
        run_ops(&engine, &mut model, &mut rng, 40);
        engine.checkpoint().unwrap();
    }
    fs::write(
        dir.path().join("indexes/INDEX_MANIFEST.json"),
        b"not valid json at all",
    )
    .unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    assert!(engine.index_load_report().rebuilt >= 1);
    verify(&engine, &model);
}
