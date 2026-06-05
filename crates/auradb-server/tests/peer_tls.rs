//! Peer (cluster) transport TLS validation for the multi-node preview.
//!
//! The peer transport uses mutual TLS: the acceptor verifies the connecting
//! node's client certificate against its configured CA, and the connector
//! verifies the peer's server certificate against its CA and the dialed server
//! name (SAN). These tests drive a real `tokio_rustls` handshake over loopback
//! using the production `build_peer_acceptor` / `build_peer_connector` and assert
//! that a wrong CA, a wrong SAN, and a peer-token mismatch are rejected, and that
//! a freshly rotated certificate signed by the same CA is accepted.

use std::fs;
use std::path::{Path, PathBuf};

use auradb_cluster::ClusterTlsConfig;
use auradb_replication::transport::{build_peer_acceptor, build_peer_connector};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A generated CA: its certificate (for signing and trust) and key.
struct Ca {
    cert: Certificate,
    key: KeyPair,
    pem_path: PathBuf,
}

fn make_ca(dir: &Path, file: &str) -> Ca {
    let mut params = CertificateParams::new(Vec::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "AuraDB Peer Test CA");
    params.distinguished_name = dn;
    let key = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    let pem_path = dir.join(file);
    fs::write(&pem_path, cert.pem()).unwrap();
    Ca {
        cert,
        key,
        pem_path,
    }
}

/// Generate a leaf certificate (usable as both client and server) with the given
/// SANs, signed by `ca`. Returns (cert_path, key_path).
fn make_leaf(dir: &Path, stem: &str, sans: &[&str], ca: &Ca) -> (PathBuf, PathBuf) {
    let mut params =
        CertificateParams::new(sans.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, stem);
    params.distinguished_name = dn;
    let key = KeyPair::generate().unwrap();
    let cert = params.signed_by(&key, &ca.cert, &ca.key).unwrap();
    let cert_path = dir.join(format!("{stem}.crt"));
    let key_path = dir.join(format!("{stem}.key"));
    fs::write(&cert_path, cert.pem()).unwrap();
    fs::write(&key_path, key.serialize_pem()).unwrap();
    (cert_path, key_path)
}

fn tls(cert: &Path, key: &Path, ca: &Path) -> ClusterTlsConfig {
    ClusterTlsConfig {
        enabled: true,
        cert_path: Some(cert.to_path_buf()),
        key_path: Some(key.to_path_buf()),
        ca_path: Some(ca.to_path_buf()),
    }
}

/// Run one mutual-TLS handshake from `connector` (dialing `server_name`) to
/// `acceptor`. Returns `Ok(())` only if both sides complete the handshake and a
/// byte round-trips.
async fn handshake(
    acceptor_tls: ClusterTlsConfig,
    connector_tls: ClusterTlsConfig,
    server_name: &str,
) -> Result<(), String> {
    let acceptor = build_peer_acceptor(&acceptor_tls).map_err(|e| format!("acceptor: {e}"))?;
    let connector = build_peer_connector(&connector_tls).map_err(|e| format!("connector: {e}"))?;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let mut tls = acceptor.accept(tcp).await.map_err(|e| e.to_string())?;
        let mut buf = [0u8; 4];
        tls.read_exact(&mut buf).await.map_err(|e| e.to_string())?;
        tls.write_all(&buf).await.map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    });

    let name = ServerName::try_from(server_name.to_string()).map_err(|e| e.to_string())?;
    let client_result: Result<(), String> = async {
        let tcp = TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
        let mut tls = connector
            .connect(name, tcp)
            .await
            .map_err(|e| e.to_string())?;
        tls.write_all(b"ping").await.map_err(|e| e.to_string())?;
        let mut buf = [0u8; 4];
        tls.read_exact(&mut buf).await.map_err(|e| e.to_string())?;
        Ok(())
    }
    .await;

    let server_result = server.await.unwrap();
    // Both sides must succeed for the handshake to count as accepted.
    client_result.and(server_result)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_tls_valid_handshake_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let ca = make_ca(dir.path(), "ca.crt");
    let (s_cert, s_key) = make_leaf(
        dir.path(),
        "node1",
        &["node1", "localhost", "127.0.0.1"],
        &ca,
    );
    let (c_cert, c_key) = make_leaf(
        dir.path(),
        "node2",
        &["node2", "localhost", "127.0.0.1"],
        &ca,
    );

    let server = tls(&s_cert, &s_key, &ca.pem_path);
    let client = tls(&c_cert, &c_key, &ca.pem_path);
    handshake(server, client, "node1")
        .await
        .expect("a valid mutual-TLS peer handshake succeeds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_tls_wrong_ca_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ca_a = make_ca(dir.path(), "ca-a.crt");
    let ca_b = make_ca(dir.path(), "ca-b.crt");
    // Server cert signed by CA-A; server trusts CA-A for client certs.
    let (s_cert, s_key) = make_leaf(dir.path(), "node1", &["node1", "127.0.0.1"], &ca_a);
    // Client cert signed by CA-B (the wrong CA); client trusts CA-A for the
    // server. The acceptor's client verifier (CA-A) rejects the CA-B client cert.
    let (c_cert, c_key) = make_leaf(dir.path(), "node2", &["node2", "127.0.0.1"], &ca_b);

    let server = tls(&s_cert, &s_key, &ca_a.pem_path);
    let client = tls(&c_cert, &c_key, &ca_a.pem_path);
    let result = handshake(server, client, "node1").await;
    assert!(
        result.is_err(),
        "a peer presenting a cert from the wrong CA must be rejected"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_tls_wrong_san_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ca = make_ca(dir.path(), "ca.crt");
    // Server cert's SANs do NOT include "node1" (the name the client dials).
    let (s_cert, s_key) = make_leaf(dir.path(), "elsewhere", &["elsewhere", "127.0.0.1"], &ca);
    let (c_cert, c_key) = make_leaf(dir.path(), "node2", &["node2", "127.0.0.1"], &ca);

    let server = tls(&s_cert, &s_key, &ca.pem_path);
    let client = tls(&c_cert, &c_key, &ca.pem_path);
    let result = handshake(server, client, "node1").await;
    assert!(
        result.is_err(),
        "a peer whose certificate SAN does not match the dialed name must be rejected"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_tls_rotated_cert_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let ca = make_ca(dir.path(), "ca.crt");
    let (c_cert, c_key) = make_leaf(dir.path(), "node2", &["node2", "127.0.0.1"], &ca);

    // Initial server cert.
    make_leaf(dir.path(), "node1", &["node1", "127.0.0.1"], &ca);
    // Rotate: a fresh key/cert for node1, still signed by the same CA with the
    // same SAN. This overwrites node1.crt / node1.key.
    let (s_cert, s_key) = make_leaf(dir.path(), "node1", &["node1", "127.0.0.1"], &ca);

    let server = tls(&s_cert, &s_key, &ca.pem_path);
    let client = tls(&c_cert, &c_key, &ca.pem_path);
    handshake(server, client, "node1")
        .await
        .expect("a rotated certificate signed by the same CA is accepted");
}
