//! Server configuration: TOML file with sensible defaults and CLI overrides.
//!
//! Configuration is validated before the server opens. Validation fails closed:
//! a security setting that is accepted is always enforced, and an invalid or
//! incomplete security setting aborts startup rather than degrading silently.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use auradb_core::{Error, Result};
use serde::{Deserialize, Serialize};

/// TLS transport configuration.
///
/// When [`TlsConfig::enabled`] is true the server terminates TLS itself using
/// the supplied certificate and key. Missing or invalid material aborts startup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    /// Whether the listener terminates TLS.
    pub enabled: bool,
    /// Path to the PEM-encoded server certificate chain.
    pub cert_path: Option<PathBuf>,
    /// Path to the PEM-encoded private key (PKCS#8 or RSA/SEC1).
    pub key_path: Option<PathBuf>,
    /// Path to a PEM-encoded CA bundle used to verify client certificates.
    pub client_ca_path: Option<PathBuf>,
    /// Whether clients must present a certificate trusted by `client_ca_path`
    /// (mutual TLS).
    pub require_client_cert: bool,
}

/// The supported authentication mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    /// A single shared static token, verified against a stored hash.
    #[default]
    StaticToken,
}

/// The supported token-hash algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenHashAlgorithm {
    /// Argon2id (memory-hard password hashing).
    #[default]
    Argon2id,
}

/// Authentication configuration.
///
/// When [`AuthConfig::enabled`] is true, clients must authenticate before any
/// schema, query, mutation, cursor, explain, transaction, or admin operation.
/// The token is never stored in plaintext: [`AuthConfig::token_hash`] holds an
/// Argon2id PHC hash produced by `auradb auth hash-token`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Whether authentication is required.
    pub enabled: bool,
    /// The authentication mechanism (only `static-token` is implemented).
    pub mode: AuthMode,
    /// The Argon2id PHC hash of the accepted token.
    pub token_hash: Option<String>,
    /// The hash algorithm (only `argon2id` is implemented).
    pub token_hash_algorithm: TokenHashAlgorithm,
}

/// The complete server configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Bind address.
    pub bind: String,
    /// Listen port.
    pub port: u16,
    /// Data directory.
    pub data_dir: PathBuf,
    /// Maximum accepted payload size in bytes.
    pub max_payload_bytes: usize,
    /// Log level / env-filter directive.
    pub log_level: String,
    /// Emit logs as JSON.
    pub log_json: bool,
    /// Cursor idle timeout in seconds.
    pub cursor_timeout_secs: u64,
    /// Default query page size.
    pub page_size: usize,
    /// Fsync the storage log after each commit.
    pub sync_on_commit: bool,
    /// Whether metrics collection is enabled.
    pub metrics_enabled: bool,
    /// Allow binding to a non-loopback address with authentication disabled.
    ///
    /// Off by default: binding a public interface without authentication is
    /// rejected unless this is explicitly set (or `--allow-insecure-bind` is
    /// passed), so a server is never unintentionally exposed unauthenticated.
    #[serde(skip_serializing_if = "is_false")]
    pub allow_insecure_bind: bool,
    /// TLS configuration.
    pub tls: TlsConfig,
    /// Authentication configuration.
    pub auth: AuthConfig,
    /// MVCC / version garbage-collection configuration.
    #[serde(default)]
    pub mvcc: MvccConfig,
    /// Cluster (Raft) configuration. Disabled by default; when disabled the
    /// server behaves exactly as the single-node engine.
    #[serde(default)]
    pub cluster: auradb_cluster::ClusterConfig,
}

/// MVCC version garbage-collection configuration (`[mvcc]`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MvccConfig {
    /// Run background garbage collection periodically.
    #[serde(default = "default_gc_enabled")]
    pub gc_enabled: bool,
    /// Interval between background GC runs, in seconds.
    #[serde(default = "default_gc_interval_secs")]
    pub gc_interval_secs: u64,
    /// Minimum number of most-recent versions of each live record GC retains.
    #[serde(default = "default_min_retained_versions")]
    pub min_retained_versions: usize,
    /// Idle timeout after which an unfinished transaction is reaped: its
    /// snapshot is released so GC can progress and further operations on it are
    /// rejected with a transaction-timeout error. `0` disables timeouts.
    #[serde(default = "default_transaction_timeout_secs")]
    pub transaction_timeout_secs: u64,
    /// Interval between abandoned-transaction reaper passes, in seconds.
    #[serde(default = "default_abandoned_transaction_reaper_secs")]
    pub abandoned_transaction_reaper_secs: u64,
}

fn default_gc_enabled() -> bool {
    true
}
fn default_gc_interval_secs() -> u64 {
    300
}
fn default_min_retained_versions() -> usize {
    1
}
fn default_transaction_timeout_secs() -> u64 {
    300
}
fn default_abandoned_transaction_reaper_secs() -> u64 {
    30
}

impl Default for MvccConfig {
    fn default() -> Self {
        MvccConfig {
            gc_enabled: default_gc_enabled(),
            gc_interval_secs: default_gc_interval_secs(),
            min_retained_versions: default_min_retained_versions(),
            transaction_timeout_secs: default_transaction_timeout_secs(),
            abandoned_transaction_reaper_secs: default_abandoned_transaction_reaper_secs(),
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bind: "127.0.0.1".to_string(),
            port: 7171,
            data_dir: PathBuf::from(".local/auradb"),
            max_payload_bytes: auradb_protocol::DEFAULT_MAX_PAYLOAD,
            log_level: "info".to_string(),
            log_json: false,
            cursor_timeout_secs: 300,
            page_size: 100,
            sync_on_commit: true,
            metrics_enabled: true,
            allow_insecure_bind: false,
            tls: TlsConfig::default(),
            auth: AuthConfig::default(),
            mvcc: MvccConfig::default(),
            cluster: auradb_cluster::ClusterConfig::default(),
        }
    }
}

impl Config {
    /// The socket address (`bind:port`).
    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.bind, self.port)
    }

    /// Load configuration from a TOML file.
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)?;
        Config::from_toml(&text)
    }

    /// Parse configuration from TOML text.
    pub fn from_toml(text: &str) -> Result<Config> {
        toml::from_str(text).map_err(|e| Error::Config(format!("invalid config: {e}")))
    }

    /// Serialize to TOML text.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config serializes")
    }

    /// Whether the configured bind address is a non-loopback (public) interface.
    pub fn is_public_bind(&self) -> bool {
        let host = self.bind.trim();
        if host.eq_ignore_ascii_case("localhost") {
            return false;
        }
        match host.parse::<IpAddr>() {
            Ok(ip) => !ip.is_loopback(),
            // A hostname other than localhost is treated as public.
            Err(_) => true,
        }
    }

    /// Validate the configuration, failing closed on unsupported or unsafe
    /// requests. TLS material referenced by the config must exist on disk.
    pub fn validate(&self) -> Result<()> {
        self.validate_inner(true)
    }

    /// Validate the configuration's structure without checking that referenced
    /// TLS files exist on disk. Useful for validating a deployment template (for
    /// example `examples/auradb.secure.toml`) whose certificate and key live on
    /// the target host. Every other check (auth hash shape, enabled-without-cert
    /// path, insecure public bind) still applies, so an invalid secure config
    /// never passes silently.
    pub fn validate_structural(&self) -> Result<()> {
        self.validate_inner(false)
    }

    fn validate_inner(&self, check_files: bool) -> Result<()> {
        if self.port == 0 {
            return Err(Error::Config("port must be non-zero".into()));
        }
        if self.max_payload_bytes == 0 {
            return Err(Error::Config("max_payload_bytes must be non-zero".into()));
        }
        if self.page_size == 0 {
            return Err(Error::Config("page_size must be non-zero".into()));
        }
        if self.mvcc.gc_enabled && self.mvcc.gc_interval_secs == 0 {
            return Err(Error::Config(
                "mvcc.gc_interval_secs must be non-zero when gc_enabled".into(),
            ));
        }
        if self.mvcc.transaction_timeout_secs > 0
            && self.mvcc.abandoned_transaction_reaper_secs == 0
        {
            return Err(Error::Config(
                "mvcc.abandoned_transaction_reaper_secs must be non-zero when \
                 transaction_timeout_secs is set"
                    .into(),
            ));
        }

        self.validate_auth()?;
        self.validate_tls(check_files)?;
        self.validate_cluster()?;

        if self.is_public_bind() && !self.auth.enabled && !self.allow_insecure_bind {
            return Err(Error::Config(format!(
                "refusing to bind non-loopback address {} with authentication disabled; \
                 enable [auth] or pass --allow-insecure-bind to override",
                self.bind
            )));
        }
        Ok(())
    }

    fn validate_auth(&self) -> Result<()> {
        if !self.auth.enabled {
            return Ok(());
        }
        // Only static-token / argon2id are implemented; the enums make any other
        // value unrepresentable, but we assert here so the intent is explicit.
        let AuthMode::StaticToken = self.auth.mode;
        let TokenHashAlgorithm::Argon2id = self.auth.token_hash_algorithm;
        let hash = self.auth.token_hash.as_deref().ok_or_else(|| {
            Error::Config(
                "auth.enabled is true but auth.token_hash is not set; \
                 generate one with `auradb auth hash-token`"
                    .into(),
            )
        })?;
        crate::auth::validate_hash(hash)?;
        Ok(())
    }

    fn validate_cluster(&self) -> Result<()> {
        // A disabled cluster section is inert; never affects single-node behavior.
        if !self.cluster.enabled {
            return Ok(());
        }
        self.cluster
            .validate()
            .map_err(|e| Error::Config(e.to_string()))?;
        // Multi-node server deployment is experimental and not enabled in this
        // release: fail closed rather than appearing to form a cluster.
        if self.cluster.is_multi_node() {
            return Err(Error::Config(
                "multi-node cluster deployment is experimental and not enabled in this release; \
                 run a single-node cluster (no peers) or disable [cluster]. The Raft and \
                 replication core is validated by in-process tests; see docs/CLUSTERING.md"
                    .into(),
            ));
        }
        // Cluster traffic has no authentication story yet: refuse a non-loopback
        // cluster bind unless the operator explicitly accepts the risk.
        if !self.cluster.is_loopback() && !self.allow_insecure_bind {
            return Err(Error::Config(format!(
                "refusing to bind cluster listen_addr {} to a non-loopback interface: cluster \
                 transport authentication is not available in this release; bind loopback or pass \
                 --allow-insecure-bind to override",
                self.cluster.listen_addr
            )));
        }
        Ok(())
    }

    fn validate_tls(&self, check_files: bool) -> Result<()> {
        if !self.tls.enabled {
            return Ok(());
        }
        let cert = self.tls.cert_path.as_ref().ok_or_else(|| {
            Error::Config("tls.enabled is true but tls.cert_path is not set".into())
        })?;
        let key = self.tls.key_path.as_ref().ok_or_else(|| {
            Error::Config("tls.enabled is true but tls.key_path is not set".into())
        })?;
        if check_files && !cert.exists() {
            return Err(Error::Config(format!(
                "tls certificate not found: {}",
                cert.display()
            )));
        }
        if check_files && !key.exists() {
            return Err(Error::Config(format!(
                "tls private key not found: {}",
                key.display()
            )));
        }
        if self.tls.require_client_cert {
            let ca = self.tls.client_ca_path.as_ref().ok_or_else(|| {
                Error::Config(
                    "tls.require_client_cert is true but tls.client_ca_path is not set".into(),
                )
            })?;
            if check_files && !ca.exists() {
                return Err(Error::Config(format!(
                    "tls client CA bundle not found: {}",
                    ca.display()
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn toml_roundtrip() {
        let c = Config {
            port: 9999,
            log_json: true,
            ..Config::default()
        };
        let text = c.to_toml();
        let back = Config::from_toml(&text).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn partial_toml_uses_defaults() {
        let c = Config::from_toml("port = 8000\n").unwrap();
        assert_eq!(c.port, 8000);
        assert_eq!(c.bind, "127.0.0.1");
        assert!(!c.auth.enabled);
        assert!(!c.tls.enabled);
    }

    #[test]
    fn auth_config_parses_new_shape() {
        let toml = r#"
[auth]
enabled = true
mode = "static-token"
token_hash = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYQ"
token_hash_algorithm = "argon2id"
"#;
        let c = Config::from_toml(toml).unwrap();
        assert!(c.auth.enabled);
        assert_eq!(c.auth.mode, AuthMode::StaticToken);
        assert_eq!(c.auth.token_hash_algorithm, TokenHashAlgorithm::Argon2id);
    }

    #[test]
    fn auth_enabled_without_hash_fails_closed() {
        let c = Config {
            auth: AuthConfig {
                enabled: true,
                token_hash: None,
                ..AuthConfig::default()
            },
            ..Config::default()
        };
        assert!(c.validate().is_err());
    }

    #[test]
    fn auth_enabled_with_valid_hash_validates() {
        let hash = crate::auth::hash_token("secret").unwrap();
        let c = Config {
            auth: AuthConfig {
                enabled: true,
                token_hash: Some(hash),
                ..AuthConfig::default()
            },
            ..Config::default()
        };
        c.validate().unwrap();
    }

    #[test]
    fn tls_enabled_without_files_fails_closed() {
        let c = Config {
            tls: TlsConfig {
                enabled: true,
                ..TlsConfig::default()
            },
            ..Config::default()
        };
        assert!(c.validate().is_err());
    }

    #[test]
    fn public_bind_without_auth_rejected_by_default() {
        let c = Config {
            bind: "0.0.0.0".into(),
            ..Config::default()
        };
        assert!(c.validate().is_err());
    }

    #[test]
    fn public_bind_allowed_with_explicit_flag() {
        let c = Config {
            bind: "0.0.0.0".into(),
            allow_insecure_bind: true,
            ..Config::default()
        };
        c.validate().unwrap();
    }

    #[test]
    fn public_bind_allowed_with_auth() {
        let hash = crate::auth::hash_token("secret").unwrap();
        let c = Config {
            bind: "0.0.0.0".into(),
            auth: AuthConfig {
                enabled: true,
                token_hash: Some(hash),
                ..AuthConfig::default()
            },
            ..Config::default()
        };
        c.validate().unwrap();
    }

    #[test]
    fn cluster_disabled_by_default() {
        let c = Config::default();
        assert!(!c.cluster.enabled);
        c.validate().unwrap();
    }

    #[test]
    fn single_node_cluster_validates() {
        let c = Config {
            cluster: auradb_cluster::ClusterConfig::single_node(),
            ..Config::default()
        };
        c.validate().unwrap();
    }

    #[test]
    fn multi_node_cluster_fails_closed() {
        let mut cluster = auradb_cluster::ClusterConfig::single_node();
        cluster.peers = vec!["10.0.0.2:7172".into()];
        let c = Config {
            cluster,
            ..Config::default()
        };
        let err = c.validate().unwrap_err();
        assert!(err.to_string().contains("experimental"));
    }

    #[test]
    fn public_cluster_bind_requires_override() {
        let mut cluster = auradb_cluster::ClusterConfig::single_node();
        cluster.listen_addr = "0.0.0.0:7172".into();
        cluster.advertise_addr = "0.0.0.0:7172".into();
        let c = Config {
            cluster: cluster.clone(),
            ..Config::default()
        };
        assert!(c.validate().is_err());
        let ok = Config {
            cluster,
            allow_insecure_bind: true,
            ..Config::default()
        };
        ok.validate().unwrap();
    }

    #[test]
    fn loopback_addresses_are_not_public() {
        for host in ["127.0.0.1", "::1", "localhost"] {
            let c = Config {
                bind: host.into(),
                ..Config::default()
            };
            assert!(!c.is_public_bind(), "{host} should be loopback");
            c.validate().unwrap();
        }
    }
}
