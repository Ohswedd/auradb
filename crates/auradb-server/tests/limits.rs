//! Defensive resource-limit drills.
//!
//! Each test drives the request dispatch path directly (no sockets) with a
//! configuration whose limits are deliberately tight, and asserts that an
//! abusive request is refused with a structured `limit_exceeded` error while a
//! well-formed request on the same session still succeeds. The wire-level
//! `max_payload_bytes` (frame-size) bound is covered by
//! `tests/integration.rs::oversized_payload_rejected_by_server` and the protocol
//! crate's frame tests.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{Filter, FindQuery, Mutation, ReadRequest};
use auradb_core::ErrorCode;
use auradb_protocol::{ErrorPayload, Frame, Opcode, RequestId};
use auradb_server::{respond, Config, LimitsConfig, Server, ServerContext, Session};
use serde::Serialize;

/// Open a server with tight, test-friendly limits.
fn tight_server(dir: &std::path::Path) -> Server {
    Server::open(Config {
        data_dir: dir.to_path_buf(),
        limits: LimitsConfig {
            max_query_limit: 100,
            max_full_text_query_tokens: 3,
            max_document_depth: 3,
            max_vector_dimension: 4,
            max_transaction_write_set: 2,
        },
        ..Config::default()
    })
    .unwrap()
}

fn send<T: Serialize>(
    ctx: &ServerContext,
    session: &mut Session,
    opcode: Opcode,
    txn_id: u64,
    payload: &T,
) -> Frame {
    let frame = Frame::json(opcode, RequestId(1), txn_id, payload).unwrap();
    respond(ctx, session, frame)
}

fn error_code(frame: &Frame) -> Option<ErrorCode> {
    if frame.opcode != Opcode::Error {
        return None;
    }
    let payload: ErrorPayload = frame.decode_json().unwrap();
    Some(payload.code)
}

/// Create a minimal `C { id }` collection so under-limit requests can succeed.
fn create_collection(ctx: &ServerContext, session: &mut Session) {
    let schema = CollectionSchema::new("C").with_field(FieldDef {
        name: "id".into(),
        field_type: FieldType::Uuid,
        primary_key: true,
        unique: true,
        nullable: false,
        indexed: false,
    });
    let resp = send(ctx, session, Opcode::SchemaCreate, 0, &schema);
    assert_eq!(resp.opcode, Opcode::Ok);
}

#[test]
fn query_limit_bound_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    let over = ReadRequest::Find(FindQuery {
        limit: Some(101),
        ..FindQuery::new("C")
    });
    let resp = send(ctx, &mut session, Opcode::Query, 0, &over);
    assert_eq!(error_code(&resp), Some(ErrorCode::LimitExceeded));

    let under = ReadRequest::Find(FindQuery {
        limit: Some(50),
        ..FindQuery::new("C")
    });
    let resp = send(ctx, &mut session, Opcode::Query, 0, &under);
    assert_ne!(error_code(&resp), Some(ErrorCode::LimitExceeded));
}

#[test]
fn vector_dimension_limit_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();

    // A schema declaring an oversize vector field is refused.
    let schema = CollectionSchema::new("V")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("e", FieldType::Vector { dim: 5 }));
    let resp = send(ctx, &mut session, Opcode::SchemaCreate, 0, &schema);
    assert_eq!(error_code(&resp), Some(ErrorCode::LimitExceeded));

    // An oversize vector value in a written record is refused too.
    create_collection(ctx, &mut session);
    let mut fields = Document::new();
    fields.insert("id".into(), Value::Text("a".into()));
    fields.insert("e".into(), Value::Vector(vec![0.0; 5]));
    let mutation = Mutation::Insert {
        collection: "C".into(),
        fields,
    };
    let resp = send(ctx, &mut session, Opcode::Mutate, 0, &mutation);
    assert_eq!(error_code(&resp), Some(ErrorCode::LimitExceeded));
}

#[test]
fn full_text_token_limit_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    let over = ReadRequest::Find(FindQuery {
        filter: Some(Filter::ContainsText {
            field: "body".into(),
            query: "alpha beta gamma delta epsilon".into(), // 5 tokens > 3
        }),
        ..FindQuery::new("C")
    });
    let resp = send(ctx, &mut session, Opcode::Query, 0, &over);
    assert_eq!(error_code(&resp), Some(ErrorCode::LimitExceeded));
}

#[test]
fn document_depth_limit_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    // { id, deep: { b: { c: 1 } } } -> depth 4 > 3
    let mut c = Document::new();
    c.insert("c".into(), Value::Int(1));
    let mut b = Document::new();
    b.insert("b".into(), Value::Object(c));
    let mut fields = Document::new();
    fields.insert("id".into(), Value::Text("a".into()));
    fields.insert("deep".into(), Value::Object(b));
    let mutation = Mutation::Insert {
        collection: "C".into(),
        fields,
    };
    let resp = send(ctx, &mut session, Opcode::Mutate, 0, &mutation);
    assert_eq!(error_code(&resp), Some(ErrorCode::LimitExceeded));
}

#[test]
fn transaction_write_set_limit_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    let begin = send(
        ctx,
        &mut session,
        Opcode::TxnBegin,
        0,
        &serde_json::json!({}),
    );
    assert_eq!(begin.opcode, Opcode::Ok);
    let txn_id = begin.txn_id;

    for i in 0..2 {
        let mut fields = Document::new();
        fields.insert("id".into(), Value::Text(format!("k{i}")));
        let mutation = Mutation::Insert {
            collection: "C".into(),
            fields,
        };
        let resp = send(ctx, &mut session, Opcode::Mutate, txn_id, &mutation);
        assert_eq!(resp.opcode, Opcode::Ok, "staged write {i} within bound");
    }
    // The third staged write exceeds max_transaction_write_set (2).
    let mut fields = Document::new();
    fields.insert("id".into(), Value::Text("k2".into()));
    let mutation = Mutation::Insert {
        collection: "C".into(),
        fields,
    };
    let resp = send(ctx, &mut session, Opcode::Mutate, txn_id, &mutation);
    assert_eq!(error_code(&resp), Some(ErrorCode::LimitExceeded));
}

#[test]
fn cursor_page_size_limit_enforced() {
    // The configured page size is a real, validated bound.
    let bad = Config {
        page_size: 0,
        ..Config::default()
    };
    assert!(bad.validate().is_err(), "page_size = 0 must be rejected");

    // A cursor fetch may not request more rows than the query bound.
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();
    let req = serde_json::json!({ "cursor_id": 1u64, "limit": 1000usize });
    let resp = send(ctx, &mut session, Opcode::CursorFetch, 0, &req);
    assert_eq!(error_code(&resp), Some(ErrorCode::LimitExceeded));
}

#[test]
fn resource_limit_errors_are_structured() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    let over = ReadRequest::Find(FindQuery {
        limit: Some(1_000),
        ..FindQuery::new("C")
    });
    let resp = send(ctx, &mut session, Opcode::Query, 0, &over);
    assert_eq!(resp.opcode, Opcode::Error);
    let payload: ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, ErrorCode::LimitExceeded);
    // A limit violation is never an honest "retry me" signal.
    assert_ne!(payload.retryable, Some(true));
    assert!(!payload.message.is_empty());
}

#[test]
fn resource_limit_errors_do_not_close_server_unnecessarily() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    // Trigger a limit error...
    let over = ReadRequest::Find(FindQuery {
        limit: Some(10_000),
        ..FindQuery::new("C")
    });
    let resp = send(ctx, &mut session, Opcode::Query, 0, &over);
    assert_eq!(error_code(&resp), Some(ErrorCode::LimitExceeded));

    // ...the same session is still fully usable afterwards.
    let health = send(ctx, &mut session, Opcode::Health, 0, &serde_json::json!({}));
    assert_eq!(health.opcode, Opcode::HealthResult);

    let ok = ReadRequest::Find(FindQuery {
        limit: Some(10),
        ..FindQuery::new("C")
    });
    let resp = send(ctx, &mut session, Opcode::Query, 0, &ok);
    assert_eq!(resp.opcode, Opcode::QueryResult);
}

// ---------------------------------------------------------------------------
// v0.8.1 edge cases: exact-boundary behavior, config validation, error-shape
// stability, payload redaction, and the no-partial-commit guarantee.
// ---------------------------------------------------------------------------

/// A read whose `limit` is exactly the configured bound is accepted; the bound
/// is inclusive.
#[test]
fn limit_boundary_exact_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path()); // max_query_limit = 100
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    let at_bound = ReadRequest::Find(FindQuery {
        limit: Some(100),
        ..FindQuery::new("C")
    });
    let resp = send(ctx, &mut session, Opcode::Query, 0, &at_bound);
    assert_eq!(
        resp.opcode,
        Opcode::QueryResult,
        "a limit equal to the bound must be accepted"
    );
}

/// A read whose `limit` is one past the bound is refused.
#[test]
fn limit_boundary_plus_one_fails() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path()); // max_query_limit = 100
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    let past_bound = ReadRequest::Find(FindQuery {
        limit: Some(101),
        ..FindQuery::new("C")
    });
    let resp = send(ctx, &mut session, Opcode::Query, 0, &past_bound);
    assert_eq!(error_code(&resp), Some(ErrorCode::LimitExceeded));
}

/// A zero limit in the configuration is rejected at validation time, so a
/// server can never start with a bound that refuses every request.
#[test]
fn limit_zero_config_rejected() {
    let zero = Config {
        limits: LimitsConfig {
            max_query_limit: 0,
            ..LimitsConfig::default()
        },
        ..Config::default()
    };
    let err = zero
        .validate()
        .expect_err("zero query limit must be rejected");
    assert!(
        err.to_string().contains("max_query_limit"),
        "error should name the offending limit: {err}"
    );
}

/// A very large limit has no upper cap: the resource bounds are operator policy,
/// not hard-coded ceilings. This documents that an absurd value validates rather
/// than being silently clamped, so an operator who sets one owns the trade-off.
#[test]
fn limit_absurd_config_rejected_or_documented() {
    let huge = Config {
        limits: LimitsConfig {
            max_query_limit: usize::MAX,
            ..LimitsConfig::default()
        },
        ..Config::default()
    };
    assert!(
        huge.validate().is_ok(),
        "resource bounds are operator policy and carry no upper cap"
    );
}

/// The structured limit error carries a stable shape: an error opcode, the
/// `limit_exceeded` code, a non-empty message, and a non-retryable signal.
#[test]
fn limit_error_shape_stable() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    let over = ReadRequest::Find(FindQuery {
        limit: Some(500),
        ..FindQuery::new("C")
    });
    let resp = send(ctx, &mut session, Opcode::Query, 0, &over);
    assert_eq!(resp.opcode, Opcode::Error);
    let payload: ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, ErrorCode::LimitExceeded);
    assert!(!payload.message.is_empty());
    assert_ne!(payload.retryable, Some(true));
}

/// The session stays usable after a limit error; reusing the same session for a
/// well-formed request succeeds.
#[test]
fn connection_usable_after_limit_error() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path());
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    let over = ReadRequest::Find(FindQuery {
        limit: Some(9_999),
        ..FindQuery::new("C")
    });
    assert_eq!(
        error_code(&send(ctx, &mut session, Opcode::Query, 0, &over)),
        Some(ErrorCode::LimitExceeded)
    );

    let ok = ReadRequest::Find(FindQuery {
        limit: Some(1),
        ..FindQuery::new("C")
    });
    assert_eq!(
        send(ctx, &mut session, Opcode::Query, 0, &ok).opcode,
        Opcode::QueryResult
    );
}

/// A limit error never echoes the request payload. The full-text token-limit
/// error reports the token count and bound but not the query text, so a secret
/// embedded in a request cannot leak through the error.
#[test]
fn limit_error_redacts_payload() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path()); // max_full_text_query_tokens = 3
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    const SECRET: &str = "hunter2secretvalue";
    let over = ReadRequest::Find(FindQuery {
        filter: Some(Filter::ContainsText {
            field: "body".into(),
            query: format!("{SECRET} alpha beta gamma delta"), // 5 tokens > 3
        }),
        ..FindQuery::new("C")
    });
    let resp = send(ctx, &mut session, Opcode::Query, 0, &over);
    assert_eq!(error_code(&resp), Some(ErrorCode::LimitExceeded));
    let payload: ErrorPayload = resp.decode_json().unwrap();
    assert!(
        !payload.message.contains(SECRET),
        "limit error must not echo request payload: {}",
        payload.message
    );
}

/// The query-limit error names the configured bound so an operator reading it
/// learns the active ceiling without consulting the config.
#[test]
fn query_limit_error_mentions_bound() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path()); // max_query_limit = 100
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    let over = ReadRequest::Find(FindQuery {
        limit: Some(101),
        ..FindQuery::new("C")
    });
    let resp = send(ctx, &mut session, Opcode::Query, 0, &over);
    let payload: ErrorPayload = resp.decode_json().unwrap();
    assert!(
        payload.message.contains("100"),
        "query-limit error should mention the configured bound: {}",
        payload.message
    );
}

/// The document-depth error names the offending top-level field so an operator
/// can find the over-nested path without bisecting the record.
#[test]
fn document_depth_error_mentions_path() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path()); // max_document_depth = 3
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    // { id, deep: { b: { c: 1 } } } -> depth 4 > 3, deepest field is `deep`.
    let mut c = Document::new();
    c.insert("c".into(), Value::Int(1));
    let mut b = Document::new();
    b.insert("b".into(), Value::Object(c));
    let mut fields = Document::new();
    fields.insert("id".into(), Value::Text("a".into()));
    fields.insert("deep".into(), Value::Object(b));
    let mutation = Mutation::Insert {
        collection: "C".into(),
        fields,
    };
    let resp = send(ctx, &mut session, Opcode::Mutate, 0, &mutation);
    let payload: ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, ErrorCode::LimitExceeded);
    assert!(
        payload.message.contains("deep"),
        "depth error should name the deepest field: {}",
        payload.message
    );
}

/// When a staged write is refused for exceeding the write-set bound, nothing it
/// carried is committed: the transaction commits only the writes that were
/// accepted before the limit fired.
#[test]
fn transaction_write_set_limit_no_partial_commit() {
    let dir = tempfile::tempdir().unwrap();
    let server = tight_server(dir.path()); // max_transaction_write_set = 2
    let ctx = server.context();
    let mut session = Session::default();
    create_collection(ctx, &mut session);

    let begin = send(
        ctx,
        &mut session,
        Opcode::TxnBegin,
        0,
        &serde_json::json!({}),
    );
    assert_eq!(begin.opcode, Opcode::Ok);
    let txn_id = begin.txn_id;

    for i in 0..2 {
        let mut fields = Document::new();
        fields.insert("id".into(), Value::Text(format!("k{i}")));
        let resp = send(
            ctx,
            &mut session,
            Opcode::Mutate,
            txn_id,
            &Mutation::Insert {
                collection: "C".into(),
                fields,
            },
        );
        assert_eq!(resp.opcode, Opcode::Ok);
    }

    // The third staged write is refused...
    let mut over = Document::new();
    over.insert("id".into(), Value::Text("k2".into()));
    let resp = send(
        ctx,
        &mut session,
        Opcode::Mutate,
        txn_id,
        &Mutation::Insert {
            collection: "C".into(),
            fields: over,
        },
    );
    assert_eq!(error_code(&resp), Some(ErrorCode::LimitExceeded));

    // ...the transaction still commits the two accepted writes, and the refused
    // record `k2` is absent: no partial commit.
    let commit = send(
        ctx,
        &mut session,
        Opcode::TxnCommit,
        txn_id,
        &serde_json::json!({}),
    );
    assert_eq!(
        commit.opcode,
        Opcode::Ok,
        "commit of accepted writes succeeds"
    );

    let all = ReadRequest::Find(FindQuery::new("C"));
    let resp = send(ctx, &mut session, Opcode::Query, 0, &all);
    assert_eq!(resp.opcode, Opcode::QueryResult);
    let rows: serde_json::Value = resp.decode_json().unwrap();
    let body = rows.to_string();
    assert!(
        body.contains("k0") && body.contains("k1"),
        "accepted writes commit: {body}"
    );
    assert!(
        !body.contains("k2"),
        "refused write must not be committed: {body}"
    );
}
