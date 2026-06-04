//! Command implementations for the `auradb` CLI. Kept separate from `main.rs`
//! so each command can be unit-tested without spawning a process.
#![forbid(unsafe_code)]

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use auradb::core::{CollectionSchema, Document, Value};
use auradb::query::{FindQuery, Mutation};
use auradb::Engine;
use auradb_server::{Config, Server};
use serde::{Deserialize, Serialize};

/// The package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `auradb version`
pub fn cmd_version() -> String {
    format!("auradb {VERSION}")
}

/// `auradb auth hash-token` - hash a token with Argon2id for use as
/// `auth.token_hash` in the server configuration.
///
/// With `--token`, the token is taken non-interactively (useful for scripts and
/// tests). Without it, the token is read from the terminal without echoing.
pub fn cmd_auth_hash_token(token: Option<String>) -> Result<String> {
    let token = match token {
        Some(t) => t,
        None => rpassword::prompt_password("Token: ").context("reading token from terminal")?,
    };
    if token.is_empty() {
        anyhow::bail!("token must not be empty");
    }
    let hash = auradb_server::auth::hash_token(&token).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(hash)
}

/// `auradb init` - create the data directory and write a default config file.
pub fn cmd_init(data_dir: &Path, config_path: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating data dir {}", data_dir.display()))?;
    // Opening the engine initializes the manifest, catalog, and first segment.
    Engine::open(data_dir).context("initializing storage")?;
    let config = Config {
        data_dir: data_dir.to_path_buf(),
        ..Config::default()
    };
    std::fs::write(config_path, config.to_toml())
        .with_context(|| format!("writing config {}", config_path.display()))?;
    Ok(())
}

/// `auradb doctor` - validate config and data directory and report stats. The
/// report includes a redacted security summary and never prints secrets.
pub fn cmd_doctor(data_dir: &Path, config: &Config) -> Result<String> {
    config.validate().context("config validation")?;
    let mut report = String::new();
    report.push_str(&format!("data_dir: {}\n", data_dir.display()));
    report.push_str(&format!("exists: {}\n", data_dir.exists()));
    let engine = Engine::open(data_dir).context("opening engine")?;
    let stats = engine.stats();
    report.push_str(&format!("collections: {}\n", stats.collections));
    report.push_str(&format!("records: {}\n", stats.records));
    report.push_str(&format!("schema_version: {}\n", stats.schema_version));
    let load = engine.index_load_report();
    report.push_str(&format!(
        "indexes: {} loaded, {} rebuilt\n",
        load.loaded, load.rebuilt
    ));
    let checked = engine.check_consistency().context("consistency check")?;
    report.push_str(&format!(
        "index_consistency: ok ({checked} records verified)\n"
    ));
    report.push_str(&security_summary(config));
    Ok(report)
}

/// A redacted summary of the security-relevant configuration. Secrets (the
/// token hash, certificate/key contents) are never printed.
fn security_summary(config: &Config) -> String {
    let mut s = String::new();
    s.push_str(&format!("bind: {} ({})\n", config.bind, config.port));
    s.push_str(&format!(
        "public_bind: {}\n",
        if config.is_public_bind() { "yes" } else { "no" }
    ));
    s.push_str(&format!(
        "auth: {}\n",
        if config.auth.enabled {
            "enabled (static-token, argon2id)"
        } else {
            "disabled"
        }
    ));
    s.push_str(&format!(
        "auth_token_hash: {}\n",
        if config.auth.token_hash.is_some() {
            "configured (redacted)"
        } else {
            "not set"
        }
    ));
    if config.tls.enabled {
        s.push_str("tls: enabled\n");
        if config.tls.require_client_cert {
            s.push_str("mutual_tls: required\n");
        }
    } else {
        s.push_str("tls: disabled\n");
    }
    if config.is_public_bind() && !config.auth.enabled {
        s.push_str("warning: public bind without authentication (insecure)\n");
    }
    s
}

/// `auradb config validate` - load and validate a config file, failing on any
/// invalid or unsafe setting.
pub fn cmd_config_validate(config_path: &Path) -> Result<String> {
    let config =
        Config::load(config_path).with_context(|| format!("loading {}", config_path.display()))?;
    config.validate().context("invalid configuration")?;
    let mut out = String::from("configuration is valid\n");
    out.push_str(&security_summary(&config));
    Ok(out)
}

/// `auradb compatibility` - print AuraDB version, protocol version, advertised
/// capabilities, and the tested Aura Connector version.
pub fn cmd_compatibility() -> String {
    use auradb::core::Capability;
    let caps: Vec<&str> = Capability::implemented()
        .iter()
        .map(|c| match c {
            Capability::PersistentStorage => "persistent_storage",
            Capability::Transactions => "transactions",
            Capability::SecondaryIndexes => "secondary_indexes",
            Capability::DocumentFields => "document_fields",
            Capability::VectorExactSearch => "vector_exact_search",
            Capability::Relationships => "relationships",
            Capability::ServerCursors => "server_cursors",
            Capability::Explain => "explain",
            Capability::MigrationEstimate => "migration_estimate",
            Capability::Observability => "observability",
            Capability::Authentication => "authentication",
            Capability::Tls => "tls",
            Capability::PersistedIndexes => "persisted_indexes",
            Capability::DocumentPathIndexes => "document_path_indexes",
            Capability::FullTextSearch => "full_text_search",
        })
        .collect();
    format!(
        "AuraDB {ver}\n\
         Aura Wire Protocol: AWP {proto}\n\
         Aura Connector (tested): 0.3.x\n\
         Capabilities: {caps}\n\
         See docs/COMPATIBILITY.md for the full matrix.",
        ver = VERSION,
        proto = auradb_protocol::PROTOCOL_VERSION,
        caps = caps.join(", "),
    )
}

/// `auradb check` - verify index consistency.
pub fn cmd_check(data_dir: &Path) -> Result<String> {
    let engine = Engine::open(data_dir)?;
    let checked = engine.check_consistency()?;
    Ok(format!("index consistency OK; {checked} records verified"))
}

/// `auradb index check` - report how indexes loaded and verify their
/// consistency against stored records.
pub fn cmd_index_check(data_dir: &Path) -> Result<String> {
    let engine = Engine::open(data_dir)?;
    let report = engine.index_load_report();
    let checked = engine.check_consistency()?;
    Ok(format!(
        "indexes: {} loaded from snapshot, {} rebuilt from storage; \
         consistency OK ({checked} records verified)",
        report.loaded, report.rebuilt
    ))
}

/// `auradb index rebuild` - rebuild every index from storage and persist fresh
/// snapshots.
pub fn cmd_index_rebuild(data_dir: &Path) -> Result<String> {
    let engine = Engine::open(data_dir)?;
    let report = engine.rebuild_indexes()?;
    Ok(format!(
        "rebuilt {} index set(s) from storage and persisted snapshots",
        report.rebuilt
    ))
}

/// `auradb compact` - compact storage.
pub fn cmd_compact(data_dir: &Path) -> Result<String> {
    let engine = Engine::open(data_dir)?;
    let report = engine.compact()?;
    Ok(format!(
        "compacted {} segment(s) into {}; {} live records retained",
        report.segments_before, report.segments_after, report.live_records
    ))
}

/// A line in a dump file: either a schema or a record.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DumpLine {
    Schema {
        schema: CollectionSchema,
    },
    Record {
        collection: String,
        fields: Document,
    },
}

/// `auradb dump` - export schemas and records as JSONL. Returns the line count.
pub fn cmd_dump(data_dir: &Path, out: &Path) -> Result<usize> {
    let engine = Engine::open(data_dir)?;
    let file = std::fs::File::create(out)
        .with_context(|| format!("creating dump file {}", out.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    let mut lines = 0;
    let schemas = engine.list_schemas();
    for schema in &schemas {
        let line = DumpLine::Schema {
            schema: schema.clone(),
        };
        serde_json::to_writer(&mut writer, &line)?;
        writer.write_all(b"\n")?;
        lines += 1;
    }
    for schema in &schemas {
        for row in engine.find(&FindQuery::new(&schema.name))? {
            let line = DumpLine::Record {
                collection: schema.name.clone(),
                fields: row.fields,
            };
            serde_json::to_writer(&mut writer, &line)?;
            writer.write_all(b"\n")?;
            lines += 1;
        }
    }
    writer.flush()?;
    Ok(lines)
}

/// `auradb restore` - load schemas and records from a JSONL dump. Returns the
/// number of records restored.
pub fn cmd_restore(data_dir: &Path, input: &Path) -> Result<usize> {
    let engine = Engine::open(data_dir)?;
    let file = std::fs::File::open(input)
        .with_context(|| format!("opening dump file {}", input.display()))?;
    let reader = std::io::BufReader::new(file);
    let mut records = 0;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let parsed: DumpLine = serde_json::from_str(&line).context("parsing dump line")?;
        match parsed {
            DumpLine::Schema { schema } => {
                engine.create_schema(schema)?;
            }
            DumpLine::Record { collection, fields } => {
                engine.apply_mutation(Mutation::Upsert { collection, fields })?;
                records += 1;
            }
        }
    }
    Ok(records)
}

/// `auradb server` - start the network server until Ctrl-C.
pub async fn cmd_server(config: Config) -> Result<()> {
    auradb_observability::init_tracing(&config.log_level, config.log_json);
    let server = Server::open(config).context("opening server")?;
    server
        .run(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("server run")?;
    Ok(())
}

/// `auradb status` - connect to a running server and report health. Supports
/// authenticating with a token and connecting over TLS.
pub async fn cmd_status(
    addr: &str,
    token: Option<String>,
    tls_ca: Option<PathBuf>,
    server_name: &str,
) -> Result<String> {
    use auradb_conformance::{ClientTls, ConnectOptions};
    let opts = ConnectOptions {
        auth_token: token,
        tls: tls_ca.map(|ca| ClientTls {
            ca_cert_path: ca,
            server_name: server_name.to_string(),
        }),
    };
    let mut client = auradb_conformance::Client::connect_with(addr, opts)
        .await
        .with_context(|| format!("connecting to {addr}"))?;
    client.ping().await.context("ping")?;
    let health = client.health().await.context("health")?;
    Ok(format!(
        "status: {:?}\nready: {}\nversion: {}\ncollections: {}",
        health.status, health.ready, health.version, health.collections
    ))
}

/// `auradb cert generate-dev` - generate a self-signed development CA and a
/// server certificate (SAN localhost/127.0.0.1) signed by it. The output is
/// suitable for local TLS testing only.
pub fn cmd_cert_generate_dev(out_dir: &Path) -> Result<String> {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
        IsCa, KeyPair, KeyUsagePurpose,
    };

    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let mut ca_params = CertificateParams::new(Vec::new()).map_err(|e| anyhow::anyhow!("{e}"))?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.use_authority_key_identifier_extension = true;
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "AuraDB Development CA");
    ca_params.distinguished_name = ca_dn;
    let ca_key = KeyPair::generate().map_err(|e| anyhow::anyhow!("{e}"))?;
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut srv_params =
        CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    srv_params.use_authority_key_identifier_extension = true;
    srv_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    srv_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let mut srv_dn = DistinguishedName::new();
    srv_dn.push(DnType::CommonName, "localhost");
    srv_params.distinguished_name = srv_dn;
    let srv_key = KeyPair::generate().map_err(|e| anyhow::anyhow!("{e}"))?;
    let srv_cert = srv_params
        .signed_by(&srv_key, &ca_cert, &ca_key)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let ca_path = out_dir.join("ca.crt");
    let ca_key_path = out_dir.join("ca.key");
    let cert_path = out_dir.join("server.crt");
    let key_path = out_dir.join("server.key");
    std::fs::write(&ca_path, ca_cert.pem())?;
    std::fs::write(&ca_key_path, ca_key.serialize_pem())?;
    std::fs::write(&cert_path, srv_cert.pem())?;
    std::fs::write(&key_path, srv_key.serialize_pem())?;
    restrict_key_permissions(&ca_key_path);
    restrict_key_permissions(&key_path);

    Ok(format!(
        "WARNING: self-signed development certificates. Do not use them in production.\n\
         wrote:\n  {ca}\n  {ca_key}\n  {cert}\n  {key}\n\n\
         Enable TLS in the server config:\n  [tls]\n  enabled = true\n  \
         cert_path = \"{cert}\"\n  key_path = \"{key}\"\n\n\
         Point clients at the CA with {ca} (server name: localhost).",
        ca = ca_path.display(),
        ca_key = ca_key_path.display(),
        cert = cert_path.display(),
        key = key_path.display(),
    ))
}

#[cfg(unix)]
fn restrict_key_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_key_permissions(_path: &Path) {}

/// `auradb bench` - run a local insert/read/vector benchmark.
pub fn cmd_bench(data_dir: &Path, records: usize) -> Result<String> {
    use auradb::core::{FieldDef, FieldType};
    use auradb::query::VectorSearch;

    let engine = Engine::open(data_dir)?;
    let schema = CollectionSchema::new("Bench")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 8 }));
    engine.create_schema(schema)?;

    let write_start = Instant::now();
    for i in 0..records {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("b{i}")));
        let v: Vec<f32> = (0..8).map(|j| ((i + j) % 17) as f32).collect();
        f.insert("embedding".into(), Value::Vector(v));
        engine.insert("Bench", f)?;
    }
    let write_elapsed = write_start.elapsed();

    let read_start = Instant::now();
    let rows = engine.find(&FindQuery::new("Bench"))?;
    let read_elapsed = read_start.elapsed();

    let vec_start = Instant::now();
    let mut q = FindQuery::new("Bench");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![1.0; 8],
        k: 10,
        metric: "cosine".into(),
    });
    let _ = engine.find(&q)?;
    let vec_elapsed = vec_start.elapsed();

    let writes_per_sec = records as f64 / write_elapsed.as_secs_f64().max(1e-9);
    Ok(format!(
        "bench results ({records} records):\n  inserts: {:.0} ops/s ({:?} total)\n  full scan: {} rows in {:?}\n  vector top-10: {:?}",
        writes_per_sec, write_elapsed, rows.len(), read_elapsed, vec_elapsed
    ))
}

/// Build a [`Config`] from an optional file plus CLI overrides.
pub fn build_config(
    config_path: Option<&Path>,
    data_dir: Option<PathBuf>,
    bind: Option<String>,
    port: Option<u16>,
    allow_insecure_bind: bool,
) -> Result<Config> {
    let mut config = match config_path {
        Some(path) => Config::load(path).with_context(|| format!("loading {}", path.display()))?,
        None => Config::default(),
    };
    if let Some(d) = data_dir {
        config.data_dir = d;
    }
    if let Some(b) = bind {
        config.bind = b;
    }
    if let Some(p) = port {
        config.port = p;
    }
    config.allow_insecure_bind = config.allow_insecure_bind || allow_insecure_bind;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_dir_and_config() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let cfg = dir.path().join("AuraDB.toml");
        cmd_init(&data, &cfg).unwrap();
        assert!(data.exists());
        assert!(cfg.exists());
        let config = Config {
            data_dir: data.clone(),
            ..Config::default()
        };
        let report = cmd_doctor(&data, &config).unwrap();
        assert!(report.contains("index_consistency: ok"));
    }

    #[test]
    fn dump_then_restore_roundtrips() {
        use auradb::core::{FieldDef, FieldType};
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        {
            let engine = Engine::open(&src).unwrap();
            engine
                .create_schema(CollectionSchema::new("C").with_field(FieldDef {
                    name: "id".into(),
                    field_type: FieldType::Uuid,
                    primary_key: true,
                    unique: true,
                    nullable: false,
                    indexed: false,
                }))
                .unwrap();
            for i in 0..3 {
                let mut f = Document::new();
                f.insert("id".into(), Value::Text(format!("r{i}")));
                engine.insert("C", f).unwrap();
            }
        }
        let dump = dir.path().join("dump.jsonl");
        let lines = cmd_dump(&src, &dump).unwrap();
        assert_eq!(lines, 1 + 3);

        let dst = dir.path().join("dst");
        let restored = cmd_restore(&dst, &dump).unwrap();
        assert_eq!(restored, 3);
        let engine = Engine::open(&dst).unwrap();
        assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 3);
    }

    #[test]
    fn check_and_compact() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        cmd_init(&data, &dir.path().join("c.toml")).unwrap();
        assert!(cmd_check(&data).unwrap().contains("OK"));
        assert!(cmd_compact(&data).unwrap().contains("compacted"));
    }

    #[test]
    fn bench_runs() {
        let dir = tempfile::tempdir().unwrap();
        let out = cmd_bench(dir.path(), 50).unwrap();
        assert!(out.contains("ops/s"));
    }

    #[test]
    fn hash_token_produces_verifiable_argon2id_hash() {
        let hash = cmd_auth_hash_token(Some("dev-secret".into())).unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(!hash.contains("dev-secret"));
        assert!(auradb_server::auth::verify_token(&hash, "dev-secret").unwrap());
        assert!(!auradb_server::auth::verify_token(&hash, "wrong").unwrap());
    }

    #[test]
    fn hash_token_rejects_empty() {
        assert!(cmd_auth_hash_token(Some(String::new())).is_err());
    }

    #[test]
    fn config_validate_accepts_default_rejects_insecure() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("ok.toml");
        cmd_init(&dir.path().join("data"), &cfg).unwrap();
        assert!(cmd_config_validate(&cfg).unwrap().contains("valid"));

        let bad = dir.path().join("bad.toml");
        std::fs::write(&bad, "bind = \"0.0.0.0\"\nport = 7171\n").unwrap();
        assert!(cmd_config_validate(&bad).is_err());
    }

    #[test]
    fn compatibility_reports_versions_and_capabilities() {
        let out = cmd_compatibility();
        assert!(out.contains(VERSION));
        assert!(out.contains("AWP"));
        assert!(out.contains("authentication"));
        assert!(out.contains("full_text_search"));
    }

    #[test]
    fn doctor_redacts_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        cmd_init(&data, &dir.path().join("c.toml")).unwrap();
        let hash = auradb_server::auth::hash_token("super-secret").unwrap();
        let config = auradb_server::Config {
            data_dir: data.clone(),
            auth: auradb_server::AuthConfig {
                enabled: true,
                token_hash: Some(hash.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let report = cmd_doctor(&data, &config).unwrap();
        assert!(report.contains("auth: enabled"));
        assert!(report.contains("redacted"));
        assert!(
            !report.contains(&hash),
            "the token hash must not be printed"
        );
        assert!(!report.contains("super-secret"));
    }

    #[test]
    fn cert_generate_dev_creates_usable_files() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("certs");
        let report = cmd_cert_generate_dev(&out).unwrap();
        assert!(report.contains("WARNING"));
        for f in ["ca.crt", "ca.key", "server.crt", "server.key"] {
            assert!(out.join(f).exists(), "{f} should be written");
        }
        // The generated server cert/key must load into the server's TLS stack.
        let scfg = auradb_server::Config {
            data_dir: dir.path().join("data"),
            tls: auradb_server::TlsConfig {
                enabled: true,
                cert_path: Some(out.join("server.crt")),
                key_path: Some(out.join("server.key")),
                ..Default::default()
            },
            ..Default::default()
        };
        auradb_server::Server::open(scfg).expect("generated cert should load");
    }
}
