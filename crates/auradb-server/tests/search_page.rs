//! Wire-level ranked pagination (`ReadRequest::SearchPage`, v1.2.0).
//!
//! Drives the request dispatch path directly (no sockets): seeds a ranked
//! collection, pages a BM25 search through opaque cursor tokens, and verifies the
//! page sequence reconstructs the single-shot ranked order with no duplicates,
//! that `page_size` is honored, and that malformed/mismatched cursors are
//! rejected with a structured `invalid_request` error without dropping the
//! session.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{
    FindQuery, Mutation, RankedPageResult, ReadRequest, SearchPageRequest, TextOperator, TextRank,
    TextSearch,
};
use auradb_core::ErrorCode;
use auradb_protocol::{ErrorPayload, Frame, Opcode, RequestId};
use auradb_server::{respond, Config, Server, ServerContext, Session};
use serde::Serialize;

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

fn error_code(frame: &Frame) -> Option<ErrorCode> {
    if frame.opcode != Opcode::Error {
        return None;
    }
    let payload: ErrorPayload = frame.decode_json().unwrap();
    Some(payload.code)
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
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 2 }))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        });
    assert_eq!(
        send(ctx, session, Opcode::SchemaCreate, &schema).opcode,
        Opcode::Ok
    );

    for i in 0..14 {
        let alphas = "alpha ".repeat(1 + (i % 4));
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("d{i:02}")));
        f.insert(
            "body".into(),
            Value::Text(format!("{alphas} beta gamma {i}")),
        );
        f.insert("embedding".into(), Value::Vector(vec![i as f32, 1.0]));
        let m = Mutation::Insert {
            collection: "Doc".into(),
            fields: f,
        };
        assert_eq!(send(ctx, session, Opcode::Mutate, &m).opcode, Opcode::Ok);
    }
}

fn bm25() -> FindQuery {
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

fn ids_of(rows: &[auradb::query::Row]) -> Vec<String> {
    rows.iter()
        .map(|r| match r.fields.get("id") {
            Some(Value::Text(s)) => s.clone(),
            _ => String::new(),
        })
        .collect()
}

#[test]
fn search_page_pages_bm25_without_duplicates_over_the_wire() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path());
    let ctx = srv.context();
    let mut session = Session::default();
    seed(ctx, &mut session);

    // Reference: a single-shot Find returns the full ranked order in one page
    // (default page_size is large).
    let find = ReadRequest::Find(bm25());
    let resp = send(ctx, &mut session, Opcode::Query, &find);
    assert_eq!(resp.opcode, Opcode::QueryResult);
    let page: auradb::query::QueryResultPage = resp.decode_json().unwrap();
    let reference = ids_of(&page.rows);
    assert_eq!(reference.len(), 14);

    // Page through via SearchPage tokens with page_size 4.
    let mut got = Vec::new();
    let mut cursor: Option<String> = None;
    let mut expected_rank = 1usize;
    loop {
        let req = ReadRequest::SearchPage(SearchPageRequest {
            find: bm25(),
            page_size: 4,
            cursor: cursor.clone(),
        });
        let resp = send(ctx, &mut session, Opcode::Query, &req);
        assert_eq!(resp.opcode, Opcode::QueryResult);
        let result: RankedPageResult = resp.decode_json().unwrap();
        assert!(result.rows.len() <= 4, "page_size honored");
        for r in &result.rows {
            assert_eq!(r.rank, Some(expected_rank), "stable cross-page rank");
            expected_rank += 1;
        }
        got.extend(ids_of(&result.rows));
        match result.next_cursor {
            Some(tok) => cursor = Some(tok),
            None => break,
        }
    }
    assert_eq!(got, reference, "wire paging reconstructs the ranked order");
    let mut dedup = got.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(
        dedup.len(),
        got.len(),
        "no duplicate rows across wire pages"
    );
}

#[test]
fn search_page_invalid_cursor_rejected_session_survives() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path());
    let ctx = srv.context();
    let mut session = Session::default();
    seed(ctx, &mut session);

    let bad = ReadRequest::SearchPage(SearchPageRequest {
        find: bm25(),
        page_size: 4,
        cursor: Some("garbage".into()),
    });
    let resp = send(ctx, &mut session, Opcode::Query, &bad);
    assert_eq!(error_code(&resp), Some(ErrorCode::InvalidRequest));

    // A page_size of 0 is rejected too.
    let zero = ReadRequest::SearchPage(SearchPageRequest {
        find: bm25(),
        page_size: 0,
        cursor: None,
    });
    assert_eq!(
        error_code(&send(ctx, &mut session, Opcode::Query, &zero)),
        Some(ErrorCode::InvalidRequest)
    );

    // The session stays usable: a valid first page still works.
    let ok = ReadRequest::SearchPage(SearchPageRequest {
        find: bm25(),
        page_size: 4,
        cursor: None,
    });
    let resp = send(ctx, &mut session, Opcode::Query, &ok);
    assert_eq!(resp.opcode, Opcode::QueryResult);
    let result: RankedPageResult = resp.decode_json().unwrap();
    assert_eq!(result.rows.len(), 4);
}

#[test]
fn search_page_non_ranked_query_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path());
    let ctx = srv.context();
    let mut session = Session::default();
    seed(ctx, &mut session);

    // A plain (non-ranked) find cannot be paged as a ranked cursor.
    let req = ReadRequest::SearchPage(SearchPageRequest {
        find: FindQuery::new("Doc"),
        page_size: 4,
        cursor: None,
    });
    assert_eq!(
        error_code(&send(ctx, &mut session, Opcode::Query, &req)),
        Some(ErrorCode::InvalidRequest)
    );
}
