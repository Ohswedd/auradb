//! Search and ranking observability: the per-shape search counters and the
//! ranking-latency histogram increment on the dispatch path for BM25, hybrid, and
//! exact vector queries, and the metrics never carry the raw query payload.

use std::sync::atomic::Ordering;

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{
    FindQuery, HybridSearch, HybridWeights, Mutation, ReadRequest, TextOperator, TextRank,
    TextSearch, VectorSearch,
};
use auradb_server::{respond, Config, Server, ServerContext, Session};
use serde::Serialize;

use auradb_protocol::{Frame, Opcode, RequestId};

fn server(dir: &std::path::Path) -> Server {
    Server::open(Config {
        data_dir: dir.to_path_buf(),
        ..Config::default()
    })
    .unwrap()
}

fn send<T: Serialize>(
    ctx: &ServerContext,
    session: &mut Session,
    opcode: Opcode,
    payload: &T,
) -> Frame {
    let frame = Frame::json(opcode, RequestId(1), 0, payload).unwrap();
    respond(ctx, session, frame)
}

fn seed(ctx: &ServerContext, session: &mut Session) {
    let schema = CollectionSchema::new("Doc")
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
        });
    assert_eq!(
        send(ctx, session, Opcode::SchemaCreate, &schema).opcode,
        Opcode::Ok
    );
    for (id, body, vec) in [
        ("d1", "raft consensus raft", vec![1.0, 0.0, 0.0]),
        ("d2", "the raft module replicas", vec![0.0, 1.0, 0.0]),
    ] {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(id.into()));
        f.insert("body".into(), Value::Text(body.into()));
        f.insert("embedding".into(), Value::Vector(vec));
        let m = Mutation::Insert {
            collection: "Doc".into(),
            fields: f,
        };
        assert_eq!(send(ctx, session, Opcode::Mutate, &m).opcode, Opcode::Ok);
    }
}

fn text_query() -> ReadRequest {
    let mut q = FindQuery::new("Doc");
    q.text_search = Some(Box::new(TextSearch {
        field: "body".into(),
        query: "raft".into(),
        operator: TextOperator::Or,
        rank: TextRank::Bm25,
        k1: None,
        b: None,
    }));
    ReadRequest::Find(q)
}

fn hybrid_query() -> ReadRequest {
    let mut q = FindQuery::new("Doc");
    q.hybrid = Some(Box::new(HybridSearch {
        text_field: "body".into(),
        text_query: "raft".into(),
        vector_field: "embedding".into(),
        vector: vec![1.0, 0.0, 0.0],
        top_k: 5,
        metric: None,
        weights: HybridWeights::default(),
        fusion: auradb::query::FusionMode::WeightedSum,
        operator: TextOperator::Or,
        k1: None,
        b: None,
    }));
    ReadRequest::Find(q)
}

fn vector_query() -> ReadRequest {
    let mut q = FindQuery::new("Doc");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![1.0, 0.0, 0.0],
        k: 2,
        metric: "cosine".into(),
    });
    ReadRequest::Find(q)
}

#[test]
fn search_metrics_increment_per_shape() {
    let dir = tempfile::tempdir().unwrap();
    let server = server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();
    seed(ctx, &mut session);

    let m = &ctx.metrics;
    assert_eq!(m.search_text_queries_total.load(Ordering::Relaxed), 0);

    // BM25 ranked text search.
    assert_eq!(
        send(ctx, &mut session, Opcode::Query, &text_query()).opcode,
        Opcode::QueryResult
    );
    assert_eq!(m.search_text_queries_total.load(Ordering::Relaxed), 1);

    // Hybrid search.
    assert_eq!(
        send(ctx, &mut session, Opcode::Query, &hybrid_query()).opcode,
        Opcode::QueryResult
    );
    assert_eq!(m.search_hybrid_queries_total.load(Ordering::Relaxed), 1);

    // Exact vector search.
    assert_eq!(
        send(ctx, &mut session, Opcode::Query, &vector_query()).opcode,
        Opcode::QueryResult
    );
    assert_eq!(m.search_vector_queries_total.load(Ordering::Relaxed), 1);

    // The ranking-latency histogram recorded the ranked queries.
    let snap = m.snapshot();
    assert!(snap.ranking_latency.count >= 3, "ranking latency recorded");

    // A non-ranked query (no clause) does not bump the search counters.
    let plain = ReadRequest::Find(FindQuery::new("Doc"));
    assert_eq!(
        send(ctx, &mut session, Opcode::Query, &plain).opcode,
        Opcode::QueryResult
    );
    assert_eq!(m.search_text_queries_total.load(Ordering::Relaxed), 1);
    assert_eq!(m.search_hybrid_queries_total.load(Ordering::Relaxed), 1);
    assert_eq!(m.search_vector_queries_total.load(Ordering::Relaxed), 1);
}

#[test]
fn search_metrics_render_no_query_payload() {
    let dir = tempfile::tempdir().unwrap();
    let server = server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();
    seed(ctx, &mut session);
    // Run a query whose text is a distinctive token that is not any metric name.
    let secret = "zzqxsecrettoken";
    let mut q = FindQuery::new("Doc");
    q.text_search = Some(Box::new(TextSearch {
        field: "body".into(),
        query: secret.into(),
        operator: TextOperator::Or,
        rank: TextRank::Bm25,
        k1: None,
        b: None,
    }));
    send(ctx, &mut session, Opcode::Query, &ReadRequest::Find(q));
    // The Prometheus exposition is counters/histograms only — never the query
    // text or any field value.
    let rendered = ctx.metrics.snapshot().render_prometheus();
    assert!(rendered.contains("auradb_search_text_queries_total"));
    assert!(rendered.contains("auradb_ranking_latency_us"));
    assert!(
        !rendered.contains(secret),
        "metrics must not echo the query text"
    );
}
