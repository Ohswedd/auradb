//! TLS listener support built on rustls.
//!
//! When TLS is enabled the server terminates TLS itself: each accepted
//! connection is wrapped in a rustls server session before any AWP framing is
//! read. Construction fails closed, so a missing, malformed, or unusable
//! certificate or key aborts server startup rather than serving plaintext.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use auradb_core::{Error, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{RootCertStore, ServerConfig};
use tokio_rustls::TlsAcceptor;

use crate::config::TlsConfig;

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let bytes = fs::read(path)
        .map_err(|e| Error::Config(format!("reading TLS certificate {}: {e}", path.display())))?;
    let certs = rustls_pemfile::certs(&mut &bytes[..])
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| Error::Config(format!("parsing TLS certificate {}: {e}", path.display())))?;
    if certs.is_empty() {
        return Err(Error::Config(format!(
            "no certificates found in {}",
            path.display()
        )));
    }
    Ok(certs)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let bytes = fs::read(path)
        .map_err(|e| Error::Config(format!("reading TLS key {}: {e}", path.display())))?;
    rustls_pemfile::private_key(&mut &bytes[..])
        .map_err(|e| Error::Config(format!("parsing TLS key {}: {e}", path.display())))?
        .ok_or_else(|| Error::Config(format!("no private key found in {}", path.display())))
}

/// Build a TLS acceptor from validated configuration.
///
/// When `require_client_cert` is set, a mutual-TLS client certificate verifier
/// is configured from `client_ca_path`; rustls then rejects any client that
/// does not present a certificate trusted by that bundle.
pub fn build_acceptor(cfg: &TlsConfig) -> Result<TlsAcceptor> {
    let cert_path = cfg
        .cert_path
        .as_ref()
        .ok_or_else(|| Error::Config("tls.cert_path is required".into()))?;
    let key_path = cfg
        .key_path
        .as_ref()
        .ok_or_else(|| Error::Config("tls.key_path is required".into()))?;
    let certs = load_certs(cert_path)?;
    let key = load_key(key_path)?;

    let builder = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Config(format!("TLS configuration error: {e}")))?;

    let config = if cfg.require_client_cert {
        let ca_path = cfg.client_ca_path.as_ref().ok_or_else(|| {
            Error::Config("tls.require_client_cert requires tls.client_ca_path".into())
        })?;
        let mut roots = RootCertStore::empty();
        for ca in load_certs(ca_path)? {
            roots
                .add(ca)
                .map_err(|e| Error::Config(format!("invalid client CA certificate: {e}")))?;
        }
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            provider(),
        )
        .build()
        .map_err(|e| Error::Config(format!("client certificate verifier: {e}")))?;
        builder
            .with_client_cert_verifier(verifier)
            .with_single_cert(certs, key)
    } else {
        builder.with_no_client_auth().with_single_cert(certs, key)
    }
    .map_err(|e| Error::Config(format!("TLS server certificate error: {e}")))?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}
