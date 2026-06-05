//! Server integration tests: concurrent clients, malformed frames, and
//! structured error responses over the wire.

use std::sync::Arc;

use auradb_core::Error;
use auradb_protocol::{Frame, Opcode, RequestId, DEFAULT_MAX_PAYLOAD, MAGIC};
use auradb_server::{read_frame, write_frame, Config, Server, Session};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// A disconnect must roll back any open transaction through the engine so its
/// pinned MVCC snapshot is released; otherwise an abandoned transaction would
/// stall version garbage collection forever.
#[test]
fn connection_cleanup_releases_transaction_snapshot() {
    use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};

    let dir = tempfile::tempdir().unwrap();
    let server = Server::open(Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    })
    .unwrap();
    let ctx = server.context();

    // Seed one record (commit c1).
    ctx.engine
        .create_schema(
            CollectionSchema::new("C")
                .with_field(FieldDef {
                    name: "id".into(),
                    field_type: FieldType::Uuid,
                    primary_key: true,
                    unique: true,
                    nullable: false,
                    indexed: false,
                })
                .with_field(FieldDef::new("v", FieldType::Int)),
        )
        .unwrap();
    let mut r = Document::new();
    r.insert("id".into(), Value::Text("r0".into()));
    r.insert("v".into(), Value::Int(1));
    ctx.engine.insert("C", r).unwrap();

    // A connection opens a transaction (snapshot pinned at c1) and is then held
    // by a session, exactly as the dispatcher does.
    let mut session = Session::default();
    let txn = ctx.engine.begin();
    session.transactions.insert(txn.id().get(), txn);
    assert_eq!(ctx.engine.stats().active_transactions, 1);

    // A later auto-commit supersedes the record (commit c2 > c1).
    ctx.engine
        .apply_mutation(auradb::query::Mutation::Update {
            collection: "C".into(),
            filter: None,
            set: {
                let mut s = Document::new();
                s.insert("v".into(), Value::Int(2));
                s
            },
        })
        .unwrap();

    // While the snapshot is held, GC cannot reclaim the old version.
    assert_eq!(ctx.engine.gc().unwrap().versions_reclaimed, 0);

    // Simulate the connection dropping without commit/rollback.
    session.cleanup(ctx);
    assert_eq!(ctx.engine.stats().active_transactions, 0);

    // The snapshot is released, so GC now reclaims the superseded version.
    assert_eq!(ctx.engine.gc().unwrap().versions_reclaimed, 1);
    assert_eq!(ctx.engine.stats().records, 1);
}

async fn start(dir: &std::path::Path) -> (String, Arc<Notify>) {
    let config = Config {
        data_dir: dir.to_path_buf(),
        ..Config::default()
    };
    let server = Server::open(config).unwrap();
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

async fn hello(stream: &mut TcpStream) -> Frame {
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
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hello_returns_capabilities() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown) = start(dir.path()).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let resp = hello(&mut stream).await;
    assert_eq!(resp.opcode, Opcode::HelloAck);
    let ack: auradb_protocol::HelloAck = resp.decode_json().unwrap();
    assert_eq!(ack.protocol_version, 1);
    assert!(!ack.capabilities.capabilities.is_empty());
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_frame_gets_structured_error() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown) = start(dir.path()).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();

    // Send a frame with corrupted magic but otherwise valid framing length.
    let mut bytes = Frame::new(Opcode::Ping, RequestId(9), 0, b"x".to_vec()).encode();
    bytes[0] = b'Z'; // break the magic
    stream.write_all(&bytes).await.unwrap();

    // The server replies with an ERROR frame and closes.
    let resp = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp.opcode, Opcode::Error);
    let payload: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, Error::Protocol(String::new()).code());
    let _ = MAGIC;
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_clients() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown) = start(dir.path()).await;

    let mut handles = Vec::new();
    for _ in 0..8 {
        let addr = addr.clone();
        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            let _ = hello(&mut stream).await;
            // ping a few times
            for i in 0..5u8 {
                let req = Frame::new(Opcode::Ping, RequestId(i as u128 + 10), 0, vec![i]);
                write_frame(&mut stream, &req).await.unwrap();
                let resp = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(resp.opcode, Opcode::Pong);
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    shutdown.notify_one();
}

async fn request<T: serde::Serialize>(
    stream: &mut TcpStream,
    opcode: Opcode,
    txn_id: u64,
    payload: &T,
) -> Frame {
    let req = Frame::json(opcode, RequestId(777), txn_id, payload).unwrap();
    write_frame(stream, &req).await.unwrap();
    read_frame(stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap()
}

/// End-to-end proof that reads carrying a transaction id execute against the
/// transaction view over the wire: a staged insert is visible to the
/// transaction's own find but invisible to a concurrent non-transactional
/// reader until commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transactional_read_sees_staged_write_over_the_wire() {
    use auradb::query::{FindQuery, Mutation, QueryResultPage, ReadRequest};
    use auradb_core::{CollectionSchema, Document, FieldDef, FieldType, Value};

    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown) = start(dir.path()).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let _ = hello(&mut stream).await;

    // Create a minimal collection.
    let schema = CollectionSchema::new("Doc").with_field(FieldDef {
        name: "id".into(),
        field_type: FieldType::Uuid,
        primary_key: true,
        unique: true,
        nullable: false,
        indexed: false,
    });
    let resp = request(&mut stream, Opcode::SchemaCreate, 0, &schema).await;
    assert_eq!(resp.opcode, Opcode::Ok);

    // Begin a transaction; the response txn id is echoed in the frame header.
    let resp = request(&mut stream, Opcode::TxnBegin, 0, &serde_json::json!({})).await;
    assert_eq!(resp.opcode, Opcode::Ok);
    let txn_id = resp.txn_id;
    assert_ne!(txn_id, 0);

    // Stage an insert within the transaction.
    let mut fields = Document::new();
    fields.insert("id".into(), Value::Text("t1".into()));
    let resp = request(
        &mut stream,
        Opcode::Mutate,
        txn_id,
        &Mutation::Insert {
            collection: "Doc".into(),
            fields,
        },
    )
    .await;
    assert_eq!(resp.opcode, Opcode::Ok);

    // A find carrying the txn id sees the staged insert.
    let find = ReadRequest::Find(FindQuery::new("Doc"));
    let resp = request(&mut stream, Opcode::Query, txn_id, &find).await;
    assert_eq!(resp.opcode, Opcode::QueryResult);
    let page: QueryResultPage = resp.decode_json().unwrap();
    assert_eq!(page.rows.len(), 1);

    // A find with no txn id (txn_id == 0) does not.
    let resp = request(&mut stream, Opcode::Query, 0, &find).await;
    let page: QueryResultPage = resp.decode_json().unwrap();
    assert_eq!(page.rows.len(), 0);

    // After commit the write is visible non-transactionally.
    let resp = request(
        &mut stream,
        Opcode::TxnCommit,
        txn_id,
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.opcode, Opcode::Ok);
    let resp = request(&mut stream, Opcode::Query, 0, &find).await;
    let page: QueryResultPage = resp.decode_json().unwrap();
    assert_eq!(page.rows.len(), 1);

    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_payload_rejected_by_server() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    };
    config.max_payload_bytes = 32;
    let server = Server::open(config).unwrap();
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
    let big = Frame::new(Opcode::Ping, RequestId(1), 0, vec![0u8; 1024]).encode();
    stream.write_all(&big).await.unwrap();
    // Server sends an error frame then closes the connection.
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf).await;
    assert!(!buf.is_empty(), "expected an error frame");
    shutdown.notify_one();
}
