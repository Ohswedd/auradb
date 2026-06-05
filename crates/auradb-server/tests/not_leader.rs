//! `not_leader` behavior over the wire.
//!
//! Even though multi-node server deployment is not enabled in this release, the
//! leader-only write path is real: when the engine's replicated log reports that
//! this node is not the leader, a write must come back as a structured
//! `not_leader` error frame — and the connection must stay healthy (the next
//! request still gets a normal response, with auth/TLS state intact). These
//! tests drive that with a stub replicated log forced into the follower state, so
//! the server-layer mapping is validated without needing a real cluster.

use std::sync::Arc;

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::Mutation;
use auradb::ReplicatedLog;
use auradb_core::{Error, ErrorCode, Result as CoreResult};
use auradb_protocol::{Frame, Opcode, RequestId, DEFAULT_MAX_PAYLOAD};
use auradb_server::{read_frame, write_frame, Config, Server};
use auradb_storage::Batch;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// A replicated log permanently in the follower role: every write is refused
/// with a structured `not_leader` error carrying a leader hint.
struct AlwaysFollower {
    leader_hint: String,
}

impl ReplicatedLog for AlwaysFollower {
    fn replicate(&self, _batch: &Batch) -> CoreResult<u64> {
        Err(Error::NotLeader(self.leader_hint.clone()))
    }
}

async fn start_follower(dir: &std::path::Path) -> (String, Arc<Notify>) {
    let server = Server::open(Config {
        data_dir: dir.to_path_buf(),
        ..Config::default()
    })
    .unwrap();
    let ctx = server.context();
    // Schema so the mutation is otherwise valid; the follower log is what refuses
    // the write, not a schema error.
    ctx.engine
        .create_schema(
            CollectionSchema::new("C")
                .with_field(FieldDef {
                    name: "id".into(),
                    field_type: FieldType::Int,
                    primary_key: true,
                    unique: true,
                    nullable: false,
                    indexed: false,
                })
                .with_field(FieldDef::new("v", FieldType::Int)),
        )
        .unwrap();
    ctx.engine.attach_replicated_log(Arc::new(AlwaysFollower {
        leader_hint: "this node is not the leader; current leader is node 00000000000000aa"
            .to_string(),
    }));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let shutdown = Arc::new(Notify::new());
    let s2 = shutdown.clone();
    tokio::spawn(async move {
        let _ = server
            .run_on(listener, async move { s2.notified().await })
            .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (addr, shutdown)
}

async fn hello(stream: &mut TcpStream) {
    let req = Frame::json(
        Opcode::Hello,
        RequestId(1),
        0,
        &serde_json::json!({ "client_version": "test", "protocol_version": 1 }),
    )
    .unwrap();
    write_frame(stream, &req).await.unwrap();
    read_frame(stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
}

fn insert_frame(req_id: u128, id: i64) -> Frame {
    let mut fields = Document::new();
    fields.insert("id".into(), Value::Int(id));
    fields.insert("v".into(), Value::Int(id * 10));
    let mutation = Mutation::Insert {
        collection: "C".into(),
        fields,
    };
    Frame::json(Opcode::Mutate, RequestId(req_id), 0, &mutation).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_leader_write_returns_structured_error() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown) = start_follower(dir.path()).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    hello(&mut stream).await;

    write_frame(&mut stream, &insert_frame(2, 1)).await.unwrap();
    let resp = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp.opcode, Opcode::Error);
    let payload: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, ErrorCode::NotLeader);
    assert!(
        payload.message.contains("not the leader"),
        "message carries a leader hint: {}",
        payload.message
    );
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_survives_not_leader() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown) = start_follower(dir.path()).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    hello(&mut stream).await;

    // A refused write does not break the connection: a subsequent Ping still
    // gets a Pong (framing, auth, and TLS state are intact).
    write_frame(&mut stream, &insert_frame(2, 1)).await.unwrap();
    let err = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(err.opcode, Opcode::Error);

    let ping = Frame::new(Opcode::Ping, RequestId(3), 0, Vec::new());
    write_frame(&mut stream, &ping).await.unwrap();
    let pong = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pong.opcode, Opcode::Pong);
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_leader_error_contains_leader_client_addr() {
    // When the leader hint carries the leader's client address (as the multi-node
    // preview builds it from a peer's declared `client_addr`), the wire error
    // surfaces it so a client can redirect.
    let dir = tempfile::tempdir().unwrap();
    let server = Server::open(Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    })
    .unwrap();
    let ctx = server.context();
    ctx.engine
        .create_schema(
            CollectionSchema::new("C")
                .with_field(FieldDef {
                    name: "id".into(),
                    field_type: FieldType::Int,
                    primary_key: true,
                    unique: true,
                    nullable: false,
                    indexed: false,
                })
                .with_field(FieldDef::new("v", FieldType::Int)),
        )
        .unwrap();
    ctx.engine.attach_replicated_log(Arc::new(AlwaysFollower {
        leader_hint: "this node (0000000000000001) is not the leader; current leader is node \
                      00000000000000aa (client address 127.0.0.1:7373); retry the write against \
                      the leader"
            .to_string(),
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let shutdown = Arc::new(Notify::new());
    let s2 = shutdown.clone();
    tokio::spawn(async move {
        let _ = server
            .run_on(listener, async move { s2.notified().await })
            .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    hello(&mut stream).await;
    write_frame(&mut stream, &insert_frame(2, 1)).await.unwrap();
    let resp = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
    let payload: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, ErrorCode::NotLeader);
    assert!(
        payload.message.contains("client address 127.0.0.1:7373"),
        "leader client address surfaced in the message: {}",
        payload.message
    );
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_leader_unknown_leader_is_retryable() {
    // A `not_leader` response is marked retryable on the wire (the client may
    // redirect to, or wait for, the current leader), even when no leader is yet
    // known.
    let dir = tempfile::tempdir().unwrap();
    let server = Server::open(Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    })
    .unwrap();
    let ctx = server.context();
    ctx.engine
        .create_schema(
            CollectionSchema::new("C")
                .with_field(FieldDef {
                    name: "id".into(),
                    field_type: FieldType::Int,
                    primary_key: true,
                    unique: true,
                    nullable: false,
                    indexed: false,
                })
                .with_field(FieldDef::new("v", FieldType::Int)),
        )
        .unwrap();
    ctx.engine.attach_replicated_log(Arc::new(AlwaysFollower {
        leader_hint: "this node is not the leader and no leader is currently known; retry after \
                      a short backoff"
            .to_string(),
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let shutdown = Arc::new(Notify::new());
    let s2 = shutdown.clone();
    tokio::spawn(async move {
        let _ = server
            .run_on(listener, async move { s2.notified().await })
            .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    hello(&mut stream).await;
    write_frame(&mut stream, &insert_frame(2, 1)).await.unwrap();
    let resp = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
    let payload: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, ErrorCode::NotLeader);
    assert_eq!(
        payload.retryable,
        Some(true),
        "not_leader is retryable on the wire"
    );
    assert!(payload.message.contains("no leader is currently known"));
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_leader_connection_remains_usable() {
    // The same connection stays usable after a `not_leader`: a follow-up Health
    // request still returns a normal report (framing and session state intact).
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown) = start_follower(dir.path()).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    hello(&mut stream).await;

    write_frame(&mut stream, &insert_frame(2, 1)).await.unwrap();
    let err = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(err.opcode, Opcode::Error);

    let health = Frame::new(Opcode::Health, RequestId(5), 0, Vec::new());
    write_frame(&mut stream, &health).await.unwrap();
    let resp = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
    let report: auradb_protocol::HealthReport = resp.decode_json().unwrap();
    assert!(report.ready, "server is still serving after not_leader");
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_leader_known_leader_not_retry_loop() {
    // A known-leader hint returns exactly one prompt, terminal error per write —
    // the server never loops internally retrying the redirect.
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown) = start_follower(dir.path()).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    hello(&mut stream).await;

    write_frame(&mut stream, &insert_frame(2, 1)).await.unwrap();
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        read_frame(&mut stream, DEFAULT_MAX_PAYLOAD),
    )
    .await
    .expect("a known-leader not_leader returns promptly, not in a retry loop")
    .unwrap()
    .unwrap();
    let payload: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, ErrorCode::NotLeader);
    assert!(payload.message.contains("current leader is node"));
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_leader_does_not_retry_forever() {
    // Each write gets exactly one deterministic `not_leader` response — the
    // server never blocks retrying internally — so a client receives a prompt,
    // terminal error rather than a hang.
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown) = start_follower(dir.path()).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    hello(&mut stream).await;

    for req_id in 2..=4 {
        write_frame(&mut stream, &insert_frame(req_id, req_id as i64))
            .await
            .unwrap();
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            read_frame(&mut stream, DEFAULT_MAX_PAYLOAD),
        )
        .await
        .expect("response arrives promptly (no infinite retry)")
        .unwrap()
        .unwrap();
        assert_eq!(resp.opcode, Opcode::Error);
        let payload: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
        assert_eq!(payload.code, ErrorCode::NotLeader);
    }
    shutdown.notify_one();
}
