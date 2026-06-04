//! Authentication integration tests: gating, per-connection state, token
//! verification, metrics, and that secrets never appear in error frames.

use std::sync::Arc;

use auradb_observability::Metrics;
use auradb_protocol::{AuthResult, Frame, HelloAck, Opcode, RequestId, DEFAULT_MAX_PAYLOAD};
use auradb_server::{auth, AuthConfig, Config, Server};
use serde::Serialize;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

const TOKEN: &str = "correct-horse-battery-staple";

fn secured_config(dir: &std::path::Path) -> Config {
    Config {
        data_dir: dir.to_path_buf(),
        auth: AuthConfig {
            enabled: true,
            token_hash: Some(auth::hash_token(TOKEN).unwrap()),
            ..AuthConfig::default()
        },
        ..Config::default()
    }
}

async fn start(config: Config) -> (String, Arc<Notify>, Arc<Metrics>) {
    let server = Server::open(config).unwrap();
    let metrics = server.context().metrics.clone();
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
    (addr, shutdown, metrics)
}

async fn call<T: Serialize>(
    stream: &mut TcpStream,
    opcode: Opcode,
    txn_id: u64,
    value: &T,
) -> Frame {
    let frame = Frame::json(opcode, RequestId(1), txn_id, value).unwrap();
    auradb_server::write_frame(stream, &frame).await.unwrap();
    auradb_server::read_frame(stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap()
}

async fn hello(stream: &mut TcpStream, token: Option<&str>) -> HelloAck {
    let payload = serde_json::json!({
        "client_version": "test",
        "protocol_version": 1,
        "auth_token": token,
    });
    let resp = call(stream, Opcode::Hello, 0, &payload).await;
    assert_eq!(resp.opcode, Opcode::HelloAck);
    resp.decode_json().unwrap()
}

fn user_schema() -> serde_json::Value {
    serde_json::json!({
        "name": "User",
        "fields": [{
            "name": "id",
            "field_type": {"kind": "uuid"},
            "primary_key": true,
            "unique": true,
            "nullable": false
        }],
        "relationships": []
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_disabled_allows_operations() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown, _m) = start(Config {
        data_dir: dir.path().to_path_buf(),
        ..Config::default()
    })
    .await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let ack = hello(&mut stream, None).await;
    assert!(!ack.auth_required);
    assert!(ack.authenticated);
    let resp = call(&mut stream, Opcode::SchemaCreate, 0, &user_schema()).await;
    assert_eq!(resp.opcode, Opcode::Ok);
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_enabled_rejects_unauthenticated() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown, _m) = start(secured_config(dir.path())).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let ack = hello(&mut stream, None).await;
    assert!(ack.auth_required);
    assert!(!ack.authenticated);

    // Liveness and readiness are allowed without auth.
    let ping = call(&mut stream, Opcode::Ping, 0, &serde_json::json!({})).await;
    assert_eq!(ping.opcode, Opcode::Pong);
    let health = call(&mut stream, Opcode::Health, 0, &serde_json::json!({})).await;
    assert_eq!(health.opcode, Opcode::HealthResult);

    // Gated operations are refused.
    for op in [
        Opcode::SchemaCreate,
        Opcode::SchemaList,
        Opcode::Query,
        Opcode::Mutate,
    ] {
        let resp = call(&mut stream, op, 0, &user_schema()).await;
        assert_eq!(resp.opcode, Opcode::Error, "{op:?} should be gated");
        let err: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
        assert_eq!(err.code.as_str(), "unauthenticated");
    }
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_enabled_accepts_valid_token_in_hello() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown, _m) = start(secured_config(dir.path())).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let ack = hello(&mut stream, Some(TOKEN)).await;
    assert!(ack.auth_required);
    assert!(ack.authenticated);
    let resp = call(&mut stream, Opcode::SchemaCreate, 0, &user_schema()).await;
    assert_eq!(resp.opcode, Opcode::Ok);
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_via_auth_opcode() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown, _m) = start(secured_config(dir.path())).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    hello(&mut stream, None).await;
    let resp = call(
        &mut stream,
        Opcode::Auth,
        0,
        &serde_json::json!({"token": TOKEN}),
    )
    .await;
    assert_eq!(resp.opcode, Opcode::AuthResult);
    let result: AuthResult = resp.decode_json().unwrap();
    assert!(result.authenticated);
    let resp = call(&mut stream, Opcode::SchemaCreate, 0, &user_schema()).await;
    assert_eq!(resp.opcode, Opcode::Ok);
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_rejects_invalid_token_and_increments_metric() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown, metrics) = start(secured_config(dir.path())).await;
    let before = metrics.snapshot().auth_failures_total;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let ack = hello(&mut stream, Some("the-wrong-token")).await;
    assert!(
        !ack.authenticated,
        "invalid handshake token must not authenticate"
    );

    let resp = call(
        &mut stream,
        Opcode::Auth,
        0,
        &serde_json::json!({"token": "still-wrong"}),
    )
    .await;
    assert_eq!(resp.opcode, Opcode::Error);
    let err: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(err.code.as_str(), "invalid_credentials");

    let after = metrics.snapshot().auth_failures_total;
    assert!(after >= before + 2, "failed auth attempts must be counted");
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn errors_never_contain_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown, _m) = start(secured_config(dir.path())).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    hello(&mut stream, None).await;
    let canary = "SECRET-LEAK-CANARY-9c1f";
    let resp = call(
        &mut stream,
        Opcode::Auth,
        0,
        &serde_json::json!({"token": canary}),
    )
    .await;
    let raw = String::from_utf8_lossy(&resp.payload);
    assert!(
        !raw.contains(canary),
        "error frame must not echo the presented token: {raw}"
    );
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_state_is_per_connection() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown, _m) = start(secured_config(dir.path())).await;

    let mut authed = TcpStream::connect(&addr).await.unwrap();
    hello(&mut authed, Some(TOKEN)).await;

    let mut anon = TcpStream::connect(&addr).await.unwrap();
    hello(&mut anon, None).await;

    // Authenticated connection succeeds.
    let ok = call(&mut authed, Opcode::SchemaCreate, 0, &user_schema()).await;
    assert_eq!(ok.opcode, Opcode::Ok);

    // The other connection is still unauthenticated.
    let denied = call(&mut anon, Opcode::SchemaList, 0, &serde_json::json!({})).await;
    assert_eq!(denied.opcode, Opcode::Error);
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cursor_operations_require_auth() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown, _m) = start(secured_config(dir.path())).await;
    let mut stream = TcpStream::connect(&addr).await.unwrap();
    hello(&mut stream, None).await;
    let resp = call(
        &mut stream,
        Opcode::CursorFetch,
        0,
        &serde_json::json!({"cursor_id": 1, "limit": 10}),
    )
    .await;
    assert_eq!(resp.opcode, Opcode::Error);
    let err: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(err.code.as_str(), "unauthenticated");
    shutdown.notify_one();
}
