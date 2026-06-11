//! BM25 ranked full-text, hybrid text+vector ranking, and their EXPLAIN /
//! EXPLAIN ANALYZE shapes, restart persistence, and error handling. These
//! exercise the v1.1.0 search and ranking surface end-to-end through the engine.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{
    FindQuery, FusionMode, HybridSearch, HybridWeights, Strategy, TextOperator, TextRank,
    TextSearch,
};
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
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

fn doc(id: &str, body: &str, vec: Vec<f32>) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(id.into()));
    f.insert("body".into(), Value::Text(body.into()));
    f.insert("embedding".into(), Value::Vector(vec));
    f
}

fn row_id(row: &auradb::query::Row) -> String {
    match row.fields.get("id") {
        Some(Value::Text(s)) => s.clone(),
        _ => String::new(),
    }
}

fn text_query(query: &str) -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.text_search = Some(Box::new(TextSearch {
        field: "body".into(),
        query: query.into(),
        operator: TextOperator::Or,
        rank: TextRank::Bm25,
        k1: None,
        b: None,
        analyzer: None,
    }));
    q
}

fn seed_text(engine: &Engine) {
    engine.create_schema(schema()).unwrap();
    // doc "dense" mentions raft twice in a short body; "sparse" once amid filler;
    // "none" never mentions it.
    engine
        .insert(
            "Doc",
            doc("dense", "raft consensus raft", vec![1.0, 0.0, 0.0]),
        )
        .unwrap();
    engine
        .insert(
            "Doc",
            doc(
                "sparse",
                "the raft module coordinates replicas across many nodes",
                vec![0.0, 1.0, 0.0],
            ),
        )
        .unwrap();
    engine
        .insert(
            "Doc",
            doc(
                "none",
                "storage compaction and flushing",
                vec![0.0, 0.0, 1.0],
            ),
        )
        .unwrap();
}

#[test]
fn full_text_bm25_ranks_relevant_docs() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_text(&engine);
    let rows = engine.find(&text_query("raft")).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(row_id(&rows[0]), "dense");
    assert_eq!(row_id(&rows[1]), "sparse");
    // Ranked rows carry a descending score and a 1-based rank.
    assert!(rows[0].score.unwrap() > rows[1].score.unwrap());
    assert_eq!(rows[0].rank, Some(1));
    assert_eq!(rows[1].rank, Some(2));
}

#[test]
fn full_text_bm25_uses_document_frequency() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    engine
        .insert(
            "Doc",
            doc("d1", "common common common rare", vec![1.0, 0.0, 0.0]),
        )
        .unwrap();
    engine
        .insert(
            "Doc",
            doc("d2", "common common common common", vec![0.0, 1.0, 0.0]),
        )
        .unwrap();
    engine
        .insert("Doc", doc("d3", "common filler text", vec![0.0, 0.0, 1.0]))
        .unwrap();
    // "rare" has high IDF (one doc); it lifts d1 above the common-heavy d2.
    let rows = engine.find(&text_query("rare common")).unwrap();
    assert_eq!(row_id(&rows[0]), "d1");
}

#[test]
fn full_text_bm25_document_length_normalization() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    // Both contain "term" once, but the short doc should rank higher under BM25
    // length normalization.
    engine
        .insert("Doc", doc("short", "term", vec![1.0, 0.0, 0.0]))
        .unwrap();
    engine
        .insert(
            "Doc",
            doc(
                "long",
                "term padding padding padding padding padding",
                vec![0.0, 1.0, 0.0],
            ),
        )
        .unwrap();
    let rows = engine.find(&text_query("term")).unwrap();
    assert_eq!(row_id(&rows[0]), "short");
    assert!(rows[0].score.unwrap() > rows[1].score.unwrap());
}

#[test]
fn full_text_bm25_handles_punctuation_and_case() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    engine
        .insert(
            "Doc",
            doc("d1", "The Quick, Brown Fox!", vec![1.0, 0.0, 0.0]),
        )
        .unwrap();
    let rows = engine.find(&text_query("QUICK fox")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(row_id(&rows[0]), "d1");
}

#[test]
fn full_text_bm25_multi_term_query() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    engine
        .insert(
            "Doc",
            doc("both", "vector search engine", vec![1.0, 0.0, 0.0]),
        )
        .unwrap();
    engine
        .insert("Doc", doc("one", "vector database", vec![0.0, 1.0, 0.0]))
        .unwrap();
    // OR semantics: both match; the doc with more query terms ranks first.
    let rows = engine.find(&text_query("vector engine")).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(row_id(&rows[0]), "both");

    // AND semantics: only the doc containing every term matches.
    let mut q = text_query("vector engine");
    q.text_search.as_mut().unwrap().operator = TextOperator::And;
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(row_id(&rows[0]), "both");
}

#[test]
fn full_text_bm25_empty_query_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_text(&engine);
    assert!(engine.find(&text_query("   ")).is_err());
}

#[test]
fn full_text_bm25_missing_index_errors() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    // Schema without a full-text index on the field.
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
    engine.create_schema(s).unwrap();
    let mut f = Document::new();
    f.insert("id".into(), Value::Text("d1".into()));
    f.insert("body".into(), Value::Text("hello".into()));
    engine.insert("Doc", f).unwrap();
    assert!(engine.find(&text_query("hello")).is_err());
}

#[test]
fn full_text_bm25_explain_shape() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_text(&engine);
    let plan = engine.explain(&text_query("raft")).unwrap();
    assert_eq!(plan.strategy, Strategy::FullTextBm25);
    assert_eq!(plan.used_index.as_deref(), Some("body"));
    let ts = plan.text_search.expect("text_search summary present");
    assert_eq!(ts.field, "body");
    assert_eq!(ts.rank, "bm25");
    assert_eq!(ts.operator, "or");
    assert_eq!(ts.query_terms, 1);
    assert_eq!(ts.indexed_documents, 3);
}

#[test]
fn full_text_bm25_explain_analyze_shape() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_text(&engine);
    let plan = engine.explain_analyze(&text_query("raft")).unwrap();
    let analysis = plan.analysis.expect("analysis present");
    assert_eq!(analysis.matched_rows, 2);
    assert_eq!(analysis.returned_rows, 2);
    assert!(plan.text_search.unwrap().candidates.is_some());
}

#[test]
fn full_text_bm25_persisted_index_reopens() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seed_text(&engine);
        engine.checkpoint().unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    let report = engine.index_load_report();
    assert_eq!(report.rebuilt, 0, "index should load from snapshot");
    let rows = engine.find(&text_query("raft")).unwrap();
    assert_eq!(row_id(&rows[0]), "dense");
}

#[test]
fn full_text_bm25_does_not_break_contains_text() {
    use auradb::query::Filter;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_text(&engine);
    // The legacy contains_text predicate keeps boolean-AND semantics.
    let mut q = FindQuery::new("Doc");
    q.filter = Some(Filter::ContainsText {
        field: "body".into(),
        query: "raft".into(),
    });
    let rows = engine.find(&q).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(engine.explain(&q).unwrap().strategy, Strategy::FullTextScan);
}

// --- Hybrid search ---

fn hybrid(text: &str, vector: Vec<f32>, weights: HybridWeights, fusion: FusionMode) -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.hybrid = Some(Box::new(HybridSearch {
        text_field: "body".into(),
        text_query: text.into(),
        vector_field: "embedding".into(),
        vector,
        top_k: 10,
        metric: None,
        weights,
        fusion,
        operator: TextOperator::Or,
        k1: None,
        b: None,
        analyzer: None,
    }));
    q
}

fn seed_hybrid(engine: &Engine) {
    engine.create_schema(schema()).unwrap();
    // "text_match" is the strongest text hit; "vector_match" is the closest vector;
    // "both" is moderate on each.
    engine
        .insert(
            "Doc",
            doc("text_match", "alpha alpha alpha", vec![0.0, 0.0, 1.0]),
        )
        .unwrap();
    engine
        .insert(
            "Doc",
            doc("vector_match", "unrelated words", vec![1.0, 0.0, 0.0]),
        )
        .unwrap();
    engine
        .insert("Doc", doc("both", "alpha context", vec![0.9, 0.1, 0.0]))
        .unwrap();
}

#[test]
fn hybrid_search_combines_text_and_vector() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid(&engine);
    let rows = engine
        .find(&hybrid(
            "alpha",
            vec![1.0, 0.0, 0.0],
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .unwrap();
    assert!(!rows.is_empty());
    // Every fused row exposes both component scores when present.
    let both = rows.iter().find(|r| row_id(r) == "both").unwrap();
    assert!(both.text_score.is_some());
    assert!(both.vector_score.is_some());
    assert!(both.score.is_some());
    assert!(both.rank.is_some());
}

#[test]
fn hybrid_search_weighted_sum_text_heavy_then_vector_heavy() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid(&engine);
    // Text-heavy: the strong text hit wins.
    let text_heavy = engine
        .find(&hybrid(
            "alpha",
            vec![1.0, 0.0, 0.0],
            HybridWeights {
                text: 0.9,
                vector: 0.1,
            },
            FusionMode::WeightedSum,
        ))
        .unwrap();
    assert_eq!(row_id(&text_heavy[0]), "text_match");

    // Vector-heavy: the closest vector wins.
    let vector_heavy = engine
        .find(&hybrid(
            "alpha",
            vec![1.0, 0.0, 0.0],
            HybridWeights {
                text: 0.1,
                vector: 0.9,
            },
            FusionMode::WeightedSum,
        ))
        .unwrap();
    assert_eq!(row_id(&vector_heavy[0]), "vector_match");
}

#[test]
fn hybrid_search_reciprocal_rank_fusion() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid(&engine);
    let rows = engine
        .find(&hybrid(
            "alpha",
            vec![1.0, 0.0, 0.0],
            HybridWeights::default(),
            FusionMode::ReciprocalRankFusion,
        ))
        .unwrap();
    assert!(!rows.is_empty());
    // RRF should surface "both" (appears in both signals) near the top.
    let top_ids: Vec<String> = rows.iter().take(2).map(row_id).collect();
    assert!(top_ids.contains(&"both".to_string()));
}

#[test]
fn hybrid_search_dimension_mismatch_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid(&engine);
    let q = hybrid(
        "alpha",
        vec![1.0, 0.0], // wrong dimension
        HybridWeights::default(),
        FusionMode::WeightedSum,
    );
    assert!(engine.find(&q).is_err());
}

#[test]
fn hybrid_search_invalid_weights_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid(&engine);
    let q = hybrid(
        "alpha",
        vec![1.0, 0.0, 0.0],
        HybridWeights {
            text: 0.0,
            vector: 0.0,
        },
        FusionMode::WeightedSum,
    );
    assert!(engine.find(&q).is_err());
}

#[test]
fn hybrid_search_empty_text_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid(&engine);
    let q = hybrid(
        "   ",
        vec![1.0, 0.0, 0.0],
        HybridWeights::default(),
        FusionMode::WeightedSum,
    );
    assert!(engine.find(&q).is_err());
}

#[test]
fn hybrid_search_explain_shape() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid(&engine);
    let plan = engine
        .explain(&hybrid(
            "alpha",
            vec![1.0, 0.0, 0.0],
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .unwrap();
    assert_eq!(plan.strategy, Strategy::Hybrid);
    let h = plan.hybrid.expect("hybrid summary present");
    assert_eq!(h.text_field, "body");
    assert_eq!(h.vector_field, "embedding");
    assert_eq!(h.fusion, "weighted_sum");
    assert_eq!(h.text_source, "bm25:body");
    assert_eq!(h.vector_source, "exact_vector:embedding");
    assert_eq!(h.weight_text, 0.5);
    assert_eq!(h.weight_vector, 0.5);
    assert_eq!(h.top_k, 10);
}

#[test]
fn hybrid_search_explain_analyze_shape() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid(&engine);
    let plan = engine
        .explain_analyze(&hybrid(
            "alpha",
            vec![1.0, 0.0, 0.0],
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .unwrap();
    let h = plan.hybrid.expect("hybrid summary present");
    assert!(h.text_candidates.is_some());
    assert!(h.vector_candidates.is_some());
    assert!(plan.analysis.is_some());
}

#[test]
fn hybrid_search_deterministic_tie_break() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid(&engine);
    let q = hybrid(
        "alpha",
        vec![1.0, 0.0, 0.0],
        HybridWeights::default(),
        FusionMode::WeightedSum,
    );
    let first: Vec<String> = engine.find(&q).unwrap().iter().map(row_id).collect();
    let second: Vec<String> = engine.find(&q).unwrap().iter().map(row_id).collect();
    assert_eq!(first, second);
}

#[test]
fn hybrid_search_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let engine = Engine::open(dir.path()).unwrap();
        seed_hybrid(&engine);
        engine.checkpoint().unwrap();
    }
    let engine = Engine::open(dir.path()).unwrap();
    let rows = engine
        .find(&hybrid(
            "alpha",
            vec![1.0, 0.0, 0.0],
            HybridWeights {
                text: 0.9,
                vector: 0.1,
            },
            FusionMode::WeightedSum,
        ))
        .unwrap();
    assert_eq!(row_id(&rows[0]), "text_match");
}

#[test]
fn conflicting_search_clauses_rejected() {
    use auradb::query::VectorSearch;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid(&engine);
    let mut q = text_query("alpha");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![1.0, 0.0, 0.0],
        k: 5,
        metric: "cosine".into(),
    });
    assert!(engine.find(&q).is_err());
}

#[test]
fn hybrid_vector_component_matches_exact_baseline() {
    // The vector_score component of a hybrid result must equal the exact vector
    // similarity (the correctness baseline), not an approximation.
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid(&engine);
    let query = vec![1.0, 0.0, 0.0];
    // Exact vector similarities for reference (cosine).
    let cosine = |a: &[f32], b: &[f32]| -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (na * nb)
    };
    let expected = [
        ("text_match", cosine(&query, &[0.0, 0.0, 1.0])),
        ("vector_match", cosine(&query, &[1.0, 0.0, 0.0])),
        ("both", cosine(&query, &[0.9, 0.1, 0.0])),
    ];
    let rows = engine
        .find(&hybrid(
            "alpha",
            query.clone(),
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .unwrap();
    for row in &rows {
        let id = row_id(row);
        let exp = expected.iter().find(|(e, _)| *e == id).map(|(_, s)| *s);
        if let (Some(vs), Some(exp)) = (row.vector_score, exp) {
            assert!(
                (vs - exp).abs() < 1e-5,
                "hybrid vector_score {vs} != exact baseline {exp} for {id}"
            );
        }
    }
}
