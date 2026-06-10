//! Engine-level coverage that the public relevance metrics score real ranked
//! retrieval output correctly. This is the seam the `auradb search eval` harness
//! relies on: the engine returns documents in relevance order, and the metric
//! functions in `auradb::query` turn that ordering plus graded judgments into
//! MRR@k / NDCG@k / Recall@k.

use std::collections::{HashMap, HashSet};

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{
    mrr_at_k, ndcg_at_k, recall_at_k, relevant_set, FindQuery, FusionMode, HybridSearch,
    HybridWeights, Row, TextOperator, TextRank, TextSearch,
};
use auradb::Engine;

const DIM: usize = 4;

/// Build a small relevance corpus: an id, a full-text `text` field, and a small
/// exact vector. The text and vectors are chosen so topical documents cluster.
fn seed(dir: &std::path::Path) -> Engine {
    let engine = Engine::open(dir).unwrap();
    let schema = CollectionSchema::new("Doc")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("text", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: DIM }))
        .with_index(IndexDef {
            path: "text".into(),
            kind: IndexKind::FullText,
        });
    engine.create_schema(schema).unwrap();

    let docs: &[(&str, &str, [f32; DIM])] = &[
        (
            "d1",
            "backup restore fresh data directory",
            [0.9, 0.1, 0.0, 0.2],
        ),
        (
            "d2",
            "verify backup checksum before restore",
            [0.85, 0.05, 0.0, 0.25],
        ),
        (
            "d3",
            "replication leader election follower",
            [0.1, 0.9, 0.1, 0.1],
        ),
        (
            "d4",
            "bm25 relevance ranking k1 b tuning",
            [0.0, 0.1, 0.9, 0.1],
        ),
        (
            "d5",
            "hybrid search text vector fusion",
            [0.0, 0.1, 0.85, 0.15],
        ),
        (
            "d6",
            "compact segments reclaim disk space",
            [0.1, 0.05, 0.05, 0.9],
        ),
    ];
    for (id, text, vec) in docs {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text((*id).into()));
        f.insert("text".into(), Value::Text((*text).into()));
        f.insert("embedding".into(), Value::Vector(vec.to_vec()));
        engine.insert("Doc", f).unwrap();
    }
    engine.analyze().unwrap();
    engine
}

fn ranked_ids(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| match r.fields.get("id") {
            Some(Value::Text(s)) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn grades(pairs: &[(&str, u32)]) -> HashMap<String, u32> {
    pairs.iter().map(|(id, g)| (id.to_string(), *g)).collect()
}

#[test]
fn bm25_ranking_scores_perfectly_for_clear_query() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seed(dir.path());

    let mut q = FindQuery::new("Doc");
    q.limit = Some(10);
    q.text_search = Some(Box::new(TextSearch {
        field: "text".into(),
        query: "backup restore fresh data directory".into(),
        operator: TextOperator::Or,
        rank: TextRank::Bm25,
        k1: None,
        b: None,
    }));
    let rows = engine.find(&q).unwrap();
    let ranked = ranked_ids(&rows);

    let g = grades(&[("d1", 3), ("d2", 1)]);
    let relevant: HashSet<String> = relevant_set(&g);
    // The on-topic backup document ranks first.
    assert_eq!(ranked.first().map(String::as_str), Some("d1"));
    assert!((mrr_at_k(&ranked, &relevant, 10) - 1.0).abs() < 1e-9);
    assert!((recall_at_k(&ranked, &relevant, 10) - 1.0).abs() < 1e-9);
    let ndcg = ndcg_at_k(&ranked, &g, 10);
    assert!(ndcg > 0.9 && ndcg <= 1.0, "ndcg {ndcg}");
}

#[test]
fn hybrid_ranking_is_scored_by_metrics() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seed(dir.path());

    let mut q = FindQuery::new("Doc");
    q.limit = Some(10);
    q.hybrid = Some(Box::new(HybridSearch {
        text_field: "text".into(),
        text_query: "hybrid search text vector fusion".into(),
        vector_field: "embedding".into(),
        vector: vec![0.0, 0.1, 0.85, 0.15],
        top_k: 10,
        metric: Some("cosine".into()),
        weights: HybridWeights {
            text: 0.7,
            vector: 0.3,
        },
        fusion: FusionMode::WeightedSum,
        operator: TextOperator::Or,
        k1: None,
        b: None,
    }));
    let rows = engine.find(&q).unwrap();
    let ranked = ranked_ids(&rows);
    assert!(!ranked.is_empty());

    let g = grades(&[("d5", 3), ("d4", 1)]);
    let relevant = relevant_set(&g);
    // The hybrid signal puts the on-topic hybrid/vector document first.
    assert_eq!(ranked.first().map(String::as_str), Some("d5"));
    let recall = recall_at_k(&ranked, &relevant, 10);
    assert!((recall - 1.0).abs() < 1e-9, "recall {recall}");
}

#[test]
fn empty_judgments_yield_zero_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let engine = seed(dir.path());
    let mut q = FindQuery::new("Doc");
    q.text_search = Some(Box::new(TextSearch {
        field: "text".into(),
        query: "nonexistent terms zzz".into(),
        operator: TextOperator::Or,
        rank: TextRank::Bm25,
        k1: None,
        b: None,
    }));
    let ranked = ranked_ids(&engine.find(&q).unwrap());
    let empty: HashSet<String> = HashSet::new();
    assert_eq!(mrr_at_k(&ranked, &empty, 10), 0.0);
    assert_eq!(recall_at_k(&ranked, &empty, 10), 0.0);
    assert_eq!(ndcg_at_k(&ranked, &HashMap::new(), 10), 0.0);
}
