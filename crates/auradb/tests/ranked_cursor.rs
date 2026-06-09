//! Stable ranked-pagination cursor tokens (v1.2.0).
//!
//! Pages BM25, hybrid, and exact-vector ranked search through opaque keyset
//! cursor tokens and verifies the guarantees: pages reconstruct the full ranked
//! order exactly (no duplicates, no gaps), `page_size` is enforced, tie-breaks
//! are stable, tokens carry no query payload, invalid/mismatched tokens are
//! rejected, and already-paged rows never reappear after a concurrent write.

use auradb::core::{
    CollectionSchema, Document, ErrorCode, FieldDef, FieldType, IndexDef, IndexKind, Value,
};
use auradb::query::{
    FindQuery, FusionMode, HybridSearch, HybridWeights, Row, TextOperator, TextRank, TextSearch,
    VectorSearch,
};
use auradb::Engine;

const DIM: usize = 3;

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
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: DIM }))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

fn doc(id: usize, body: &str, vec: Vec<f32>) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(format!("d{id:02}")));
    f.insert("body".into(), Value::Text(body.into()));
    f.insert("embedding".into(), Value::Vector(vec));
    f
}

fn seeded(engine: &Engine, n: usize) {
    engine.create_schema(schema()).unwrap();
    for i in 0..n {
        // Vary "alpha" repetition for BM25 score spread; vary the vector too.
        let alphas = "alpha ".repeat(1 + (i % 4));
        let body = format!("{alphas} beta gamma doc {i}");
        let a = ((i % 5) as f32) / 5.0;
        engine
            .insert("Doc", doc(i, &body, vec![a, 1.0 - a, 0.5]))
            .unwrap();
    }
}

fn row_id(r: &Row) -> String {
    match r.fields.get("id") {
        Some(Value::Text(s)) => s.clone(),
        _ => String::new(),
    }
}

fn bm25_query() -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.text_search = Some(Box::new(TextSearch {
        field: "body".into(),
        query: "alpha".into(),
        operator: TextOperator::Or,
        rank: TextRank::Bm25,
        k1: None,
        b: None,
    }));
    q
}

fn vector_query(k: usize) -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![1.0, 0.0, 0.5],
        k,
        metric: "cosine".into(),
    });
    q
}

fn hybrid_query() -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.hybrid = Some(Box::new(HybridSearch {
        text_field: "body".into(),
        text_query: "alpha".into(),
        vector_field: "embedding".into(),
        vector: vec![1.0, 0.0, 0.5],
        top_k: 12,
        metric: None,
        weights: HybridWeights::default(),
        fusion: FusionMode::WeightedSum,
        operator: TextOperator::Or,
        k1: None,
        b: None,
    }));
    q
}

/// Page the whole result with `page_size`, returning the concatenated ids and
/// asserting each page is within bounds and ranks are contiguous.
fn page_all(engine: &Engine, q: &FindQuery, page_size: usize) -> Vec<String> {
    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    let mut expected_rank = 1usize;
    let mut pages = 0;
    loop {
        let (rows, next) = engine.search_page(q, page_size, cursor.as_deref()).unwrap();
        assert!(rows.len() <= page_size, "page must respect page_size");
        for r in &rows {
            // Ranks are stable and contiguous across pages.
            assert_eq!(r.rank, Some(expected_rank), "stable cross-page rank");
            expected_rank += 1;
            ids.push(row_id(r));
        }
        pages += 1;
        assert!(pages < 1000, "pagination must terminate");
        match next {
            Some(tok) => cursor = Some(tok),
            None => break,
        }
    }
    ids
}

/// The full ranked order as the single-shot query produces it (the reference).
fn reference_ids(engine: &Engine, q: &FindQuery) -> Vec<String> {
    engine.find(q).unwrap().iter().map(row_id).collect()
}

fn assert_pages_match_reference(engine: &Engine, q: &FindQuery, page_size: usize) {
    let reference = reference_ids(engine, q);
    let paged = page_all(engine, q, page_size);
    assert_eq!(
        paged, reference,
        "paged ids must equal the full ranked order"
    );
    // No duplicates.
    let mut sorted = paged.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), paged.len(), "no duplicate rows across pages");
    assert!(!reference.is_empty(), "test query should match something");
}

#[test]
fn bm25_cursor_pages_without_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine, 14);
    assert_pages_match_reference(&engine, &bm25_query(), 3);
}

#[test]
fn vector_cursor_pages_without_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine, 14);
    assert_pages_match_reference(&engine, &vector_query(14), 4);
}

#[test]
fn hybrid_cursor_pages_without_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine, 14);
    assert_pages_match_reference(&engine, &hybrid_query(), 5);
}

#[test]
fn ranked_cursor_stable_tie_breaks() {
    // Many documents share an identical vector -> identical score, so the order
    // is resolved purely by the (internal) record-id tie-break. Whatever that
    // deterministic order is, paging must reproduce it exactly with no duplicate
    // and no skipped row.
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    for i in 0..10 {
        engine
            .insert("Doc", doc(i, "alpha", vec![1.0, 0.0, 0.5]))
            .unwrap();
    }
    let q = vector_query(10);
    let reference = reference_ids(&engine, &q);
    assert_eq!(reference.len(), 10);
    // Paging reproduces the tie-broken order exactly across page boundaries.
    let paged = page_all(&engine, &q, 3);
    assert_eq!(
        paged, reference,
        "paging reproduces the stable tie-break order"
    );
    let mut dedup = paged.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), 10, "every tied row appears exactly once");
}

#[test]
fn ranked_cursor_limit_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine, 14);
    let (rows, next) = engine.search_page(&bm25_query(), 4, None).unwrap();
    assert_eq!(rows.len(), 4, "first page is exactly page_size");
    assert!(next.is_some(), "more pages remain");
    // page_size 0 is rejected.
    assert_eq!(
        engine
            .search_page(&bm25_query(), 0, None)
            .err()
            .unwrap()
            .code(),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn ranked_cursor_invalid_token_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine, 14);

    // Garbage token.
    let err = engine
        .search_page(&bm25_query(), 3, Some("not-a-real-token"))
        .err()
        .unwrap();
    assert_eq!(err.code(), ErrorCode::InvalidRequest);

    // A valid token from one query must not be accepted by a different query
    // (the fingerprint won't match).
    let (_rows, next) = engine.search_page(&bm25_query(), 3, None).unwrap();
    let token = next.expect("more pages");
    let other = vector_query(14);
    let err = engine.search_page(&other, 3, Some(&token)).err().unwrap();
    assert_eq!(err.code(), ErrorCode::InvalidRequest);

    // A non-ranked query cannot use ranked cursors.
    let err = engine
        .search_page(&FindQuery::new("Doc"), 3, None)
        .err()
        .unwrap();
    assert_eq!(err.code(), ErrorCode::InvalidRequest);
}

#[test]
fn ranked_cursor_token_redacts_query_payload() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine, 14);
    let (_rows, next) = engine.search_page(&bm25_query(), 3, None).unwrap();
    let token = next.expect("more pages");
    // The token is opaque hex and never echoes the query text or any payload.
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(
        !token.contains("alpha"),
        "token must not carry the query text"
    );
    assert!(!token.to_lowercase().contains("doc"));
}

#[test]
fn ranked_cursor_after_concurrent_write_snapshot_behavior() {
    // Exact-vector similarity is corpus-independent, so a concurrent insert does
    // not re-score existing rows: keyset paging stays duplicate-free even outside
    // a transaction. (BM25 scores depend on corpus statistics, so for stable
    // BM25/hybrid paging across writes you page inside a transaction snapshot —
    // see the module docs.)
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seeded(&engine, 12);
    let q = vector_query(20);

    let (page1, next) = engine.search_page(&q, 4, None).unwrap();
    let seen: std::collections::HashSet<String> = page1.iter().map(row_id).collect();
    assert_eq!(seen.len(), 4);

    // Insert a new matching document mid-pagination.
    engine
        .insert("Doc", doc(99, "newly added document", vec![0.9, 0.1, 0.5]))
        .unwrap();

    // Continue paging to exhaustion; no already-seen id reappears.
    let mut cursor = next;
    let mut later = Vec::new();
    while let Some(tok) = cursor {
        let (rows, n) = engine.search_page(&q, 4, Some(&tok)).unwrap();
        for r in &rows {
            let id = row_id(r);
            assert!(
                !seen.contains(&id),
                "already-paged row {id} must not reappear"
            );
            later.push(id);
        }
        cursor = n;
    }
    // The pages after page 1 are themselves duplicate-free.
    let mut dedup = later.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), later.len(), "no duplicates across later pages");
}
