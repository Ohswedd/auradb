//! Server configuration: TOML file with sensible defaults and CLI overrides.

use std::path::{Path, PathBuf};

use auradb_core::{Error, Result};
use serde::{Deserialize, Serialize};

/// TLS configuration shape. TLS is not implemented in this single-node release;
/// the shape exists so configuration is forward-compatible and `enabled = true`
/// fails closed rather than silently serving plaintext.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Whether TLS is requested.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the certificate file.
    #[serde(default)]
    pub cert_path: Option<PathBuf>,
    /// Path to the private key file.
    #[serde(default)]
    pub key_path: Option<PathBuf>,
}

/// Authentication configuration shape. Static-token auth is the only mechanism
/// shaped here; when `required` is true a token must be supplied.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Whether authentication is required.
    #[serde(default)]
    pub required: bool,
    /// Accepted static tokens (if any).
    #[serde(default)]
    pub static_tokens: Vec<String>,
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
    /// TLS configuration shape.
    pub tls: TlsConfig,
    /// Auth configuration shape.
    pub auth: AuthConfig,
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
            tls: TlsConfig::default(),
            auth: AuthConfig::default(),
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

    /// Validate the configuration, failing closed on unsupported requests.
    pub fn validate(&self) -> Result<()> {
        if self.port == 0 {
            return Err(Error::Config("port must be non-zero".into()));
        }
        if self.max_payload_bytes == 0 {
            return Err(Error::Config("max_payload_bytes must be non-zero".into()));
        }
        if self.page_size == 0 {
            return Err(Error::Config("page_size must be non-zero".into()));
        }
        if self.tls.enabled {
            return Err(Error::unsupported(
                "TLS termination (configure a TLS proxy in front of AuraDB)",
            ));
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
    }

    #[test]
    fn tls_enabled_fails_closed() {
        let c = Config {
            tls: TlsConfig {
                enabled: true,
                ..TlsConfig::default()
            },
            ..Config::default()
        };
        assert!(c.validate().is_err());
    }
}
