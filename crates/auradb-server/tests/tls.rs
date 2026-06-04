//! TLS integration tests: fail-closed validation, a real TLS round trip, plain
//! TCP rejection against a TLS listener, auth over TLS, and mutual TLS.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use auradb_protocol::{Frame, HelloAck, Opcode, RequestId, DEFAULT_MAX_PAYLOAD};
use auradb_server::{auth, read_frame, write_frame, AuthConfig, Config, Server, TlsConfig};
use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_rustls::TlsConnector;

/// Generate a dev CA plus a server certificate signed by it (SAN
/// localhost/127.0.0.1). Returns (ca.crt, server.crt, server.key) paths.
fn make_certs(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let mut ca_params = CertificateParams::new(Vec::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.use_authority_key_identifier_extension = true;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "AuraDB Test CA");
    ca_params.distinguished_name = dn;
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();

    let mut srv_params =
        CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
    srv_params.use_authority_key_identifier_extension = true;
    let srv_key = KeyPair::generate().unwrap();
    let srv_cert = srv_params.signed_by(&srv_key, &ca_cert, &ca_key).unwrap();

    let ca_path = dir.join("ca.crt");
    let cert_path = dir.join("server.crt");
    let key_path = dir.join("server.key");
    fs::write(&ca_path, ca_cert.pem()).unwrap();
    fs::write(&cert_path, srv_cert.pem()).unwrap();
    fs::write(&key_path, srv_key.serialize_pem()).unwrap();
    (ca_path, cert_path, key_path)
}

fn client_config(ca_path: &Path) -> Arc<rustls::ClientConfig> {
    let ca_bytes = fs::read(ca_path).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for c in rustls_pemfile::certs(&mut &ca_bytes[..]) {
        roots.add(c.unwrap()).unwrap();
    }
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    Arc::new(cfg)
}

async fn start(config: Config) -> (String, Arc<Notify>) {
    let server = Server::open(config).unwrap();
    assert!(server.tls_enabled());
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

async fn tls_connect(
    addr: &str,
    ca_path: &Path,
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let connector = TlsConnector::from(client_config(ca_path));
    let tcp = TcpStream::connect(addr).await?;
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    connector.connect(name, tcp).await
}

async fn hello<S>(stream: &mut S, token: Option<&str>) -> HelloAck
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let payload = serde_json::json!({
        "client_version": "test",
        "protocol_version": 1,
        "auth_token": token,
    });
    let frame = Frame::json(Opcode::Hello, RequestId(1), 0, &payload).unwrap();
    write_frame(stream, &frame).await.unwrap();
    let resp = read_frame(stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp.opcode, Opcode::HelloAck);
    resp.decode_json().unwrap()
}

fn tls_config(cert: &Path, key: &Path) -> TlsConfig {
    TlsConfig {
        enabled: true,
        cert_path: Some(cert.to_path_buf()),
        key_path: Some(key.to_path_buf()),
        ..TlsConfig::default()
    }
}

#[test]
fn tls_enabled_missing_files_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        data_dir: dir.path().to_path_buf(),
        tls: TlsConfig {
            enabled: true,
            cert_path: Some(dir.path().join("nope.crt")),
            key_path: Some(dir.path().join("nope.key")),
            ..TlsConfig::default()
        },
        ..Config::default()
    };
    assert!(Server::open(cfg).is_err());
}

#[test]
fn tls_enabled_invalid_files_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("bad.crt");
    let key = dir.path().join("bad.key");
    fs::write(&cert, b"not a certificate").unwrap();
    fs::write(&key, b"not a key").unwrap();
    let cfg = Config {
        data_dir: dir.path().to_path_buf(),
        tls: tls_config(&cert, &key),
        ..Config::default()
    };
    assert!(Server::open(cfg).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (ca, cert, key) = make_certs(dir.path());
    let cfg = Config {
        data_dir: dir.path().join("data"),
        tls: tls_config(&cert, &key),
        ..Config::default()
    };
    let (addr, shutdown) = start(cfg).await;
    let mut stream = tls_connect(&addr, &ca).await.unwrap();
    let ack = hello(&mut stream, None).await;
    assert_eq!(ack.protocol_version, 1);
    assert!(ack.capabilities.has(auradb_core::Capability::Tls));
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plain_tcp_cannot_speak_to_tls_listener() {
    let dir = tempfile::tempdir().unwrap();
    let (_ca, cert, key) = make_certs(dir.path());
    let cfg = Config {
        data_dir: dir.path().join("data"),
        tls: tls_config(&cert, &key),
        ..Config::default()
    };
    let (addr, shutdown) = start(cfg).await;

    let mut plain = TcpStream::connect(&addr).await.unwrap();
    let frame = Frame::json(
        Opcode::Hello,
        RequestId(1),
        0,
        &serde_json::json!({"client_version": "x", "protocol_version": 1}),
    )
    .unwrap();
    // Writing AWP bytes to a TLS listener is not a valid ClientHello; the server
    // aborts the TLS handshake and never returns an AWP frame.
    let _ = write_frame(&mut plain, &frame).await;
    let result = read_frame(&mut plain, DEFAULT_MAX_PAYLOAD).await;
    let got_frame = matches!(result, Ok(Some(_)));
    assert!(!got_frame, "plaintext client must not receive an AWP frame");
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_over_tls() {
    let dir = tempfile::tempdir().unwrap();
    let (ca, cert, key) = make_certs(dir.path());
    let cfg = Config {
        data_dir: dir.path().join("data"),
        tls: tls_config(&cert, &key),
        auth: AuthConfig {
            enabled: true,
            token_hash: Some(auth::hash_token("tls-token").unwrap()),
            ..AuthConfig::default()
        },
        ..Config::default()
    };
    let (addr, shutdown) = start(cfg).await;
    let mut stream = tls_connect(&addr, &ca).await.unwrap();
    let ack = hello(&mut stream, Some("tls-token")).await;
    assert!(ack.auth_required);
    assert!(ack.authenticated);

    let schema = serde_json::json!({
        "name": "User",
        "fields": [{"name": "id", "field_type": {"kind": "uuid"},
                    "primary_key": true, "unique": true, "nullable": false}],
        "relationships": []
    });
    let frame = Frame::json(Opcode::SchemaCreate, RequestId(2), 0, &schema).unwrap();
    write_frame(&mut stream, &frame).await.unwrap();
    let resp = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp.opcode, Opcode::Ok);
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutual_tls_rejects_client_without_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let (ca, cert, key) = make_certs(dir.path());
    let cfg = Config {
        data_dir: dir.path().join("data"),
        tls: TlsConfig {
            enabled: true,
            cert_path: Some(cert),
            key_path: Some(key),
            client_ca_path: Some(ca.clone()),
            require_client_cert: true,
        },
        ..Config::default()
    };
    let (addr, shutdown) = start(cfg).await;
    // A client with no certificate must be rejected during the TLS handshake.
    let mut stream = match tls_connect(&addr, &ca).await {
        Ok(s) => s,
        Err(_) => {
            shutdown.notify_one();
            return; // handshake refused at connect time: correct.
        }
    };
    // If the connector returned, the first write/read must fail rather than
    // yield an AWP frame.
    let frame = Frame::json(
        Opcode::Hello,
        RequestId(1),
        0,
        &serde_json::json!({"client_version": "x", "protocol_version": 1}),
    )
    .unwrap();
    let _ = write_frame(&mut stream, &frame).await;
    let result = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD).await;
    assert!(
        !matches!(result, Ok(Some(_))),
        "mTLS listener must reject a client without a certificate"
    );
    shutdown.notify_one();
}
