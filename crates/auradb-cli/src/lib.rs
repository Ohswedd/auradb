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

mod cluster;
pub use cluster::{
    cluster_metadata_report, cmd_cluster_bootstrap, cmd_cluster_compact_log, cmd_cluster_doctor,
    cmd_cluster_init, cmd_cluster_peers, cmd_cluster_status, ClusterDoctorReport,
    ClusterMetadataReport,
};

/// The package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The Aura Wire Protocol version this build speaks.
pub const PROTOCOL_VERSION: u8 = auradb_protocol::PROTOCOL_VERSION;

/// `auradb version`
pub fn cmd_version() -> String {
    format!("auradb {VERSION}")
}

/// Atomically write `bytes` to `path`: write a sibling temp file, fsync it, then
/// rename over the destination. A crash leaves either the old or the new file
/// intact, never a half-written one.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent() {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// The backup path for a config file (`AuraDB.toml` -> `AuraDB.toml.bak`).
fn backup_path_for(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".bak");
    path.with_file_name(name)
}

/// `auradb auth rotate-token` - replace the configured static token with a new
/// one.
///
/// The new token is hashed with Argon2id and the configuration is rewritten
/// atomically with unrelated fields preserved, then re-read and validated. The
/// plaintext token is never stored or printed. With `--backup`, the previous
/// configuration is copied to `<config>.bak` first.
///
/// A server that is already running keeps verifying against the token hash it
/// loaded at startup, and connections authenticated with the old token stay
/// authenticated until they disconnect. AuraDB does not hot-reload the token, so
/// restart the server to enforce the new token.
pub fn cmd_auth_rotate_token(
    config_path: &Path,
    token: Option<String>,
    backup: bool,
) -> Result<String> {
    let token = match token {
        Some(t) => t,
        None => rpassword::prompt_password("New token: ").context("reading token from terminal")?,
    };
    if token.is_empty() {
        anyhow::bail!("token must not be empty");
    }
    let mut config =
        Config::load(config_path).with_context(|| format!("loading {}", config_path.display()))?;

    let hash = auradb_server::auth::hash_token(&token).map_err(|e| anyhow::anyhow!("{e}"))?;
    // Defense in depth: we store a PHC hash, never the plaintext. (Substring
    // checks against short tokens are unreliable, so we assert the structural
    // shape of the hash instead; Argon2id makes the plaintext unrecoverable.)
    if !hash.starts_with("$argon2") {
        anyhow::bail!("refusing to write a token hash that is not an Argon2 PHC string");
    }
    config.auth.token_hash = Some(hash);
    // A rotated token is only meaningful with authentication enabled.
    config.auth.enabled = true;
    // Validate the new config before touching disk so a good file is never
    // replaced with an invalid one.
    config
        .validate()
        .context("the rotated configuration is invalid")?;

    if backup {
        let backup_path = backup_path_for(config_path);
        std::fs::copy(config_path, &backup_path)
            .with_context(|| format!("writing backup {}", backup_path.display()))?;
    }

    let serialized = config.to_toml();
    atomic_write(config_path, serialized.as_bytes())
        .with_context(|| format!("writing {}", config_path.display()))?;

    // Re-read from disk and validate so the persisted file is proven good.
    let reloaded = Config::load(config_path).context("reloading the written config")?;
    reloaded
        .validate()
        .context("the written config failed validation")?;

    let mut out = format!(
        "rotated auth.token_hash in {} (Argon2id; token redacted)\n",
        config_path.display()
    );
    if backup {
        out.push_str(&format!(
            "previous config backed up to {}\n",
            backup_path_for(config_path).display()
        ));
    }
    out.push_str(
        "note: a running server keeps the token it loaded at startup; \
         restart it to enforce the new token",
    );
    Ok(out)
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
    // Create a stable node identity so a node id exists (and persists) even
    // before cluster mode is enabled. This is inert while `[cluster]` is
    // disabled, but means enabling cluster mode reuses the same identity.
    auradb_cluster::ClusterStore::new(data_dir)
        .init(None, None, VERSION)
        .context("initializing node identity")?;
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
    let stale = planner_stats_stale(&engine, &stats);
    let (mvcc, warnings) = mvcc_doctor(&stats, config, true, stale);
    report.push_str(&format!(
        "mvcc: {} active txn(s), {} timed out, {} retained version(s), gc {}, timeout {}s\n",
        mvcc.active_transactions,
        mvcc.timed_out_transactions,
        mvcc.retained_versions,
        if mvcc.gc_enabled {
            "enabled"
        } else {
            "disabled"
        },
        mvcc.transaction_timeout_secs,
    ));
    if warnings.is_empty() {
        report.push_str("warnings: none\n");
    } else {
        for w in &warnings {
            report.push_str(&format!("warning: {w}\n"));
        }
    }
    report.push_str(&security_summary(config));
    // Cluster metadata validation: loading rejects an unknown future format.
    let cluster =
        cluster_metadata_report(data_dir, config).context("validating cluster metadata")?;
    report.push_str(&format!("cluster_enabled: {}\n", cluster.enabled));
    report.push_str(&format!("cluster_initialized: {}\n", cluster.initialized));
    if let Some(node_id) = &cluster.node_id {
        report.push_str(&format!("node_id: {node_id}\n"));
    }
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

/// A redacted, machine-readable summary of the security-relevant configuration.
/// No secret (token hash, certificate, or key) is ever included.
#[derive(Debug, Serialize)]
pub struct SecurityReport {
    /// The bind address.
    pub bind: String,
    /// The listen port.
    pub port: u16,
    /// Whether the bind address is a non-loopback (public) interface.
    pub public_bind: bool,
    /// Whether authentication is enabled.
    pub auth_enabled: bool,
    /// Whether a token hash is configured (the hash itself is never shown).
    pub auth_token_hash_configured: bool,
    /// Whether TLS is enabled.
    pub tls_enabled: bool,
    /// Whether mutual TLS (client certificates) is required.
    pub mutual_tls_required: bool,
    /// Whether an insecure public bind without auth is explicitly permitted.
    pub allow_insecure_bind: bool,
    /// A human-readable warning if the configuration is exposed and unauthenticated.
    pub insecure_warning: Option<String>,
}

impl SecurityReport {
    /// Build a redacted security report from a configuration.
    pub fn from_config(config: &Config) -> SecurityReport {
        let public = config.is_public_bind();
        SecurityReport {
            bind: config.bind.clone(),
            port: config.port,
            public_bind: public,
            auth_enabled: config.auth.enabled,
            auth_token_hash_configured: config.auth.token_hash.is_some(),
            tls_enabled: config.tls.enabled,
            mutual_tls_required: config.tls.require_client_cert,
            allow_insecure_bind: config.allow_insecure_bind,
            insecure_warning: if public && !config.auth.enabled {
                Some("public bind without authentication (insecure)".into())
            } else {
                None
            },
        }
    }
}

/// A machine-readable health and readiness report for a local data directory,
/// emitted by `auradb doctor --json`. Secrets are redacted.
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    /// The AuraDB version.
    pub auradb_version: String,
    /// The Aura Wire Protocol version this build speaks.
    pub protocol_version: u8,
    /// The inspected data directory.
    pub data_dir: String,
    /// Whether the data directory exists.
    pub data_dir_exists: bool,
    /// Whether the storage engine opened successfully.
    pub storage_open: bool,
    /// Number of registered collections.
    pub collections: usize,
    /// Total live records across all collections.
    pub records: usize,
    /// The schema catalog version.
    pub schema_version: u64,
    /// Collections whose indexes loaded from a persisted snapshot.
    pub indexes_loaded: usize,
    /// Collections whose indexes were rebuilt from storage.
    pub indexes_rebuilt: usize,
    /// Whether the index-vs-storage consistency check passed.
    pub index_consistency_ok: bool,
    /// The number of records verified by the consistency check.
    pub records_verified: usize,
    /// MVCC health and pressure summary.
    pub mvcc: MvccDoctor,
    /// Operational warnings (MVCC pressure, GC disabled, stale stats, etc.).
    pub warnings: Vec<String>,
    /// The redacted security summary.
    pub security: SecurityReport,
    /// Cluster metadata and configuration summary.
    pub cluster: ClusterMetadataReport,
}

/// MVCC health fields included in the doctor report.
#[derive(Debug, Serialize)]
pub struct MvccDoctor {
    /// Transactions currently holding a pinned snapshot.
    pub active_transactions: usize,
    /// Registered transactions that have timed out but not been cleaned up.
    pub timed_out_transactions: usize,
    /// Age in seconds of the oldest active transaction, if any.
    pub oldest_transaction_age_secs: Option<u64>,
    /// Total stored MVCC versions retained.
    pub retained_versions: usize,
    /// Cumulative transactions reaped for exceeding the idle timeout.
    pub transaction_timeouts_total: u64,
    /// Whether background version GC is enabled in the configuration.
    pub gc_enabled: bool,
    /// Configured transaction idle timeout in seconds (`0` = disabled).
    pub transaction_timeout_secs: u64,
}

/// Thresholds above which the doctor raises an MVCC pressure warning.
const DOCTOR_MANY_ACTIVE_TRANSACTIONS: usize = 100;
const DOCTOR_OLD_SNAPSHOT_AGE_SECS: u64 = 3600;
const DOCTOR_HIGH_VERSION_RATIO: usize = 4;

/// Build the doctor's MVCC summary and operational warnings from engine stats
/// and configuration. Warnings flag long-lived snapshots, version pressure, a
/// disabled GC, stale planner statistics, and a needed index check.
fn mvcc_doctor(
    stats: &auradb::EngineStats,
    config: &Config,
    index_consistency_ok: bool,
    stats_stale: bool,
) -> (MvccDoctor, Vec<String>) {
    let mut warnings = Vec::new();
    if stats.active_transactions > DOCTOR_MANY_ACTIVE_TRANSACTIONS {
        warnings.push(format!(
            "{} active transactions hold pinned snapshots; long-lived transactions delay version GC",
            stats.active_transactions
        ));
    }
    if let Some(age) = stats.oldest_transaction_age_secs {
        if age > DOCTOR_OLD_SNAPSHOT_AGE_SECS {
            warnings.push(format!(
                "oldest active snapshot is {age}s old; it pins versions and blocks GC below its read timestamp"
            ));
        }
    }
    if stats.timed_out_transactions > 0 {
        warnings.push(format!(
            "{} timed-out transaction(s) await cleanup",
            stats.timed_out_transactions
        ));
    }
    if stats.records > 0 && stats.versions > stats.records.saturating_mul(DOCTOR_HIGH_VERSION_RATIO)
    {
        warnings.push(format!(
            "{} retained versions for {} live records; run `auradb gc` to reclaim superseded versions",
            stats.versions, stats.records
        ));
    }
    if !config.mvcc.gc_enabled {
        warnings.push(
            "background version GC is disabled (mvcc.gc_enabled = false); versions are not reclaimed automatically".into(),
        );
    }
    if config.mvcc.transaction_timeout_secs == 0 {
        warnings.push(
            "transaction timeouts are disabled (mvcc.transaction_timeout_secs = 0); abandoned transactions pin versions indefinitely".into(),
        );
    }
    if stats_stale {
        warnings.push(
            "planner statistics look stale; run `auradb stats analyze` to refresh them".into(),
        );
    }
    if !index_consistency_ok {
        warnings.push("index consistency check failed; run `auradb index rebuild`".into());
    }
    let mvcc = MvccDoctor {
        active_transactions: stats.active_transactions,
        timed_out_transactions: stats.timed_out_transactions,
        oldest_transaction_age_secs: stats.oldest_transaction_age_secs,
        retained_versions: stats.versions,
        transaction_timeouts_total: stats.transaction_timeouts_total,
        gc_enabled: config.mvcc.gc_enabled,
        transaction_timeout_secs: config.mvcc.transaction_timeout_secs,
    };
    (mvcc, warnings)
}

/// Whether the persisted planner row counts look stale relative to the engine's
/// live record total. A coarse heuristic: stale if the totals disagree.
fn planner_stats_stale(engine: &Engine, stats: &auradb::EngineStats) -> bool {
    let planner = engine.planner_stats();
    if planner.collections.is_empty() && stats.records > 0 {
        return true;
    }
    let counted: usize = planner.collections.values().map(|c| c.row_count).sum();
    counted != stats.records
}

/// `auradb doctor --json` - the same checks as `auradb doctor`, emitted as JSON.
pub fn cmd_doctor_json(data_dir: &Path, config: &Config) -> Result<String> {
    config.validate().context("config validation")?;
    let engine = Engine::open(data_dir).context("opening engine")?;
    let stats = engine.stats();
    let load = engine.index_load_report();
    let checked = engine.check_consistency().context("consistency check")?;
    let stale = planner_stats_stale(&engine, &stats);
    let (mvcc, warnings) = mvcc_doctor(&stats, config, true, stale);
    let report = DoctorReport {
        auradb_version: VERSION.to_string(),
        protocol_version: PROTOCOL_VERSION,
        data_dir: data_dir.display().to_string(),
        data_dir_exists: data_dir.exists(),
        storage_open: true,
        collections: stats.collections,
        records: stats.records,
        schema_version: stats.schema_version,
        indexes_loaded: load.loaded,
        indexes_rebuilt: load.rebuilt,
        index_consistency_ok: true,
        records_verified: checked,
        mvcc,
        warnings,
        security: SecurityReport::from_config(config),
        cluster: cluster_metadata_report(data_dir, config)
            .context("validating cluster metadata")?,
    };
    serde_json::to_string_pretty(&report).context("serializing doctor report")
}

/// A machine-readable status report for a running server, emitted by
/// `auradb status --json`. It carries the fields the health frame exposes plus
/// the client-known protocol version and queried address.
#[derive(Debug, Serialize)]
pub struct StatusReport {
    /// The queried server address.
    pub addr: String,
    /// Whether the ping succeeded.
    pub reachable: bool,
    /// The reported health status (`healthy` or `degraded`).
    pub status: String,
    /// Whether the server is ready to serve requests.
    pub ready: bool,
    /// The server's reported version.
    pub server_version: String,
    /// The Aura Wire Protocol version negotiated by this client.
    pub protocol_version: u8,
    /// The number of collections reported by the server.
    pub collections: usize,
    /// Whether the connection used TLS.
    pub tls: bool,
    /// MVCC health and version-pressure summary, when the server reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mvcc: Option<auradb_protocol::MvccHealth>,
    /// Cluster / replication summary, when the server runs in cluster mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster: Option<auradb_protocol::ClusterHealth>,
}

/// `auradb config validate` - load and validate a config file, failing on any
/// invalid or unsafe setting.
pub fn cmd_config_validate(config_path: &Path, no_file_checks: bool) -> Result<String> {
    let config =
        Config::load(config_path).with_context(|| format!("loading {}", config_path.display()))?;
    if no_file_checks {
        config
            .validate_structural()
            .context("invalid configuration")?;
    } else {
        config.validate().context("invalid configuration")?;
    }
    let mut out = if no_file_checks {
        String::from("configuration is structurally valid (TLS files not checked)\n")
    } else {
        String::from("configuration is valid\n")
    };
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

/// A machine-readable garbage-collection result for `auradb gc --json`.
#[derive(Debug, Serialize)]
pub struct GcCliReport {
    /// Whether this was a dry run (no data was modified).
    pub dry_run: bool,
    /// Superseded/tombstone versions reclaimed (or that would be reclaimed).
    pub versions_reclaimed: usize,
    /// Record ids removed entirely (fully-dead tombstone chains).
    pub records_removed: usize,
    /// Total versions remaining after GC.
    pub versions_after: usize,
    /// On-disk bytes reclaimed. Not estimated for a dry run (reported as 0).
    pub bytes_reclaimed: u64,
}

/// `auradb gc` - reclaim old MVCC versions no active transaction can observe.
/// With `--dry-run`, report what would be reclaimed without modifying data.
pub fn cmd_gc(data_dir: &Path, dry_run: bool, json: bool) -> Result<String> {
    let engine = Engine::open(data_dir)?;
    let report = if dry_run {
        engine.gc_dry_run()
    } else {
        engine.gc()?
    };
    if json {
        let cli = GcCliReport {
            dry_run,
            versions_reclaimed: report.versions_reclaimed,
            records_removed: report.records_removed,
            versions_after: report.versions_after,
            bytes_reclaimed: report.bytes_reclaimed,
        };
        return Ok(serde_json::to_string_pretty(&cli)?);
    }
    if dry_run {
        Ok(format!(
            "dry run: would reclaim {} version(s) and remove {} deleted record(s); {} version(s) would remain (no data modified)",
            report.versions_reclaimed, report.records_removed, report.versions_after
        ))
    } else {
        Ok(format!(
            "garbage collection reclaimed {} version(s) and removed {} deleted record(s); {} version(s) retained, {} byte(s) reclaimed",
            report.versions_reclaimed, report.records_removed, report.versions_after, report.bytes_reclaimed
        ))
    }
}

/// `auradb stats analyze` - recompute and persist planner statistics.
pub fn cmd_stats_analyze(data_dir: &Path) -> Result<String> {
    let engine = Engine::open(data_dir)?;
    engine.analyze()?;
    let stats = engine.planner_stats();
    Ok(format!(
        "analyzed {} collection(s); planner statistics persisted",
        stats.collections.len()
    ))
}

/// `auradb stats show` - print current planner statistics.
pub fn cmd_stats_show(data_dir: &Path, json: bool) -> Result<String> {
    let engine = Engine::open(data_dir)?;
    let stats = engine.planner_stats();
    if json {
        return Ok(serde_json::to_string_pretty(&stats)?);
    }
    let mut out = String::new();
    if stats.collections.is_empty() {
        out.push_str("no planner statistics yet; run `auradb stats analyze`");
        return Ok(out);
    }
    for (name, c) in &stats.collections {
        out.push_str(&format!(
            "{name}: {} rows, avg {} bytes/record\n",
            c.row_count, c.avg_record_size
        ));
        for (field, distinct) in &c.field_cardinality {
            out.push_str(&format!("  {field}: {distinct} distinct value(s)\n"));
        }
        for (field, n) in &c.vector_count {
            out.push_str(&format!("  {field}: {n} vector(s)\n"));
        }
        for (field, n) in &c.text_field_docs {
            out.push_str(&format!("  {field}: {n} full-text document(s)\n"));
        }
    }
    Ok(out.trim_end().to_string())
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

/// Order schemas so that every relationship target precedes the collections
/// that reference it (a stable topological sort). Records are dumped in this
/// order so restore, which validates referential integrity as it upserts, never
/// sees a record before its target exists. Cycles or self-references fall back
/// to appending the remaining collections in their original order.
fn order_schemas_by_dependency(schemas: Vec<CollectionSchema>) -> Vec<CollectionSchema> {
    use std::collections::HashSet;
    let names: HashSet<String> = schemas.iter().map(|s| s.name.clone()).collect();
    let mut placed: HashSet<String> = HashSet::new();
    let mut ordered: Vec<CollectionSchema> = Vec::with_capacity(schemas.len());

    // Iterate to a fixed point: place any schema whose in-set relationship
    // targets are already placed. Stop when a full pass places nothing.
    let mut remaining = schemas;
    loop {
        let mut progress = false;
        let mut next_remaining = Vec::new();
        for schema in remaining {
            let ready = schema.relationships.iter().all(|r| {
                r.target == schema.name // self-reference cannot block placement
                    || !names.contains(&r.target)
                    || placed.contains(&r.target)
            });
            if ready {
                placed.insert(schema.name.clone());
                ordered.push(schema);
                progress = true;
            } else {
                next_remaining.push(schema);
            }
        }
        remaining = next_remaining;
        if remaining.is_empty() {
            break;
        }
        if !progress {
            // A cycle remains; append the rest in their original order.
            ordered.extend(remaining);
            break;
        }
    }
    ordered
}

/// `auradb dump` - export schemas and records as JSONL. Returns the line count.
pub fn cmd_dump(data_dir: &Path, out: &Path) -> Result<usize> {
    let engine = Engine::open(data_dir)?;
    let file = std::fs::File::create(out)
        .with_context(|| format!("creating dump file {}", out.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    let mut lines = 0;
    // Order collections so a relationship's target is written (and therefore
    // restored) before the collection that references it; restore validates
    // referential integrity as it upserts, so a referenced record must exist
    // first.
    let schemas = order_schemas_by_dependency(engine.list_schemas());
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

/// `auradb snapshot create` - capture the engine state as a portable snapshot
/// file. When the data directory carries cluster identity, the snapshot records
/// the cluster and node id so a later restore can detect a cluster mismatch.
pub fn cmd_snapshot_create(data_dir: &Path, output: &Path) -> Result<String> {
    use auradb_replication::SnapshotManifest;
    let engine = Engine::open(data_dir).context("opening engine")?;
    let identity = auradb_cluster::ClusterStore::new(data_dir)
        .load()
        .ok()
        .flatten();
    let (cid, nid) = match &identity {
        Some(id) => (
            Some(id.cluster_id().to_string()),
            Some(id.node_id().to_string()),
        ),
        None => (None, None),
    };
    let manifest = SnapshotManifest::create(&engine, 0, 0, VERSION)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_identity(cid, nid);
    let bytes = manifest.encode().map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(output, &bytes).with_context(|| format!("writing {}", output.display()))?;
    Ok(format!(
        "wrote snapshot to {} ({} collection(s), {} record(s))",
        output.display(),
        manifest.meta.collections,
        manifest.meta.records
    ))
}

/// `auradb snapshot inspect` - print a snapshot manifest and verify its integrity
/// without restoring it.
pub fn cmd_snapshot_inspect(input: &Path) -> Result<String> {
    use auradb_replication::SnapshotManifest;
    let bytes = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let manifest = SnapshotManifest::decode(&bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    let integrity = match manifest.verified_payload() {
        Ok(_) => "ok",
        Err(_) => "FAILED",
    };
    let m = &manifest.meta;
    Ok(format!(
        "snapshot_format_version: {}\n\
         cluster_id: {}\n\
         node_id: {}\n\
         last_included_index: {}\n\
         last_included_term: {}\n\
         storage_format_version: {}\n\
         collections: {}\n\
         records: {}\n\
         digest: {:08x}\n\
         integrity: {}\n\
         created_by_version: {}\n\
         created_at_unix: {}",
        m.format_version,
        m.cluster_id.as_deref().unwrap_or("(none)"),
        m.node_id.as_deref().unwrap_or("(none)"),
        m.last_included_index,
        m.last_included_term,
        m.storage_format_version,
        m.collections,
        m.records,
        m.digest,
        integrity,
        m.created_by_version,
        m.created_at_unix,
    ))
}

/// `auradb snapshot restore` - restore a snapshot file into a data directory.
/// Refuses to overwrite a non-empty directory unless `force` is set; the restore
/// is atomic (built in a staging directory and swapped into place).
pub fn cmd_snapshot_restore(input: &Path, data_dir: &Path, force: bool) -> Result<String> {
    use auradb_replication::{RestoreOptions, SnapshotManifest};
    let bytes = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let manifest = SnapshotManifest::decode(&bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    let opts = RestoreOptions {
        force,
        ..RestoreOptions::default()
    };
    let engine = manifest
        .restore_to(data_dir, &opts)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let records: usize = engine
        .list_schemas()
        .iter()
        .map(|s| {
            engine
                .find(&FindQuery::new(&s.name))
                .map(|r| r.len())
                .unwrap_or(0)
        })
        .sum();
    Ok(format!(
        "restored snapshot into {} ({} record(s))",
        data_dir.display(),
        records
    ))
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
    let report = status_report(addr, token, tls_ca, server_name).await?;
    let mut out = format!(
        "status: {}\nready: {}\nversion: {}\ncollections: {}",
        report.status, report.ready, report.server_version, report.collections
    );
    if let Some(m) = &report.mvcc {
        out.push_str(&format!(
            "\nactive_transactions: {}\ntimed_out_transactions: {}\noldest_snapshot_age_secs: {}\nretained_versions: {}\ntransaction_timeouts_total: {}\ntransaction_timeout_secs: {}\ngc_enabled: {}",
            m.active_transactions,
            m.timed_out_transactions,
            m.oldest_transaction_age_secs
                .map(|s| s.to_string())
                .unwrap_or_else(|| "n/a".into()),
            m.retained_versions,
            m.transaction_timeouts_total,
            m.transaction_timeout_secs,
            m.gc_enabled,
        ));
    }
    if let Some(c) = &report.cluster {
        out.push_str(&format!(
            "\ncluster_enabled: {}\nnode_id: {}\ncluster_id: {}\nrole: {}\nterm: {}\nleader_id: {}\nleader_client_addr: {}\ncommit_index: {}\napplied_index: {}\nlast_log_index: {}\npeer_count: {}\nreplication_lag_entries: {}",
            c.enabled,
            c.node_id.as_deref().unwrap_or("n/a"),
            c.cluster_id.as_deref().unwrap_or("n/a"),
            c.role,
            c.term,
            c.leader_id.as_deref().unwrap_or("n/a"),
            c.leader_client_addr.as_deref().unwrap_or("n/a"),
            c.commit_index,
            c.applied_index,
            c.last_log_index,
            c.peer_count,
            c.replication_lag_entries,
        ));
    }
    Ok(out)
}

/// `auradb status --json` - the same probe as `auradb status`, emitted as JSON.
pub async fn cmd_status_json(
    addr: &str,
    token: Option<String>,
    tls_ca: Option<PathBuf>,
    server_name: &str,
) -> Result<String> {
    let report = status_report(addr, token, tls_ca, server_name).await?;
    serde_json::to_string_pretty(&report).context("serializing status report")
}

/// `auradb cluster status --addr` — query a running server for live cluster
/// diagnostics: role, leader (and its client address when known), quorum,
/// replication indices, and per-peer reachability. This is the live counterpart
/// to the offline `auradb cluster status --data-dir` view.
pub async fn cmd_cluster_status_live(
    addr: &str,
    token: Option<String>,
    tls_ca: Option<PathBuf>,
    server_name: &str,
    json: bool,
) -> Result<String> {
    let report = status_report(addr, token, tls_ca, server_name).await?;
    let cluster = report
        .cluster
        .ok_or_else(|| anyhow::anyhow!("server at {addr} is not running in cluster mode"))?;
    if json {
        return serde_json::to_string_pretty(&cluster).context("serializing cluster status");
    }
    let mut out = String::new();
    out.push_str(&format!("addr: {addr}\n"));
    out.push_str(&format!("enabled: {}\n", cluster.enabled));
    out.push_str(&format!(
        "node_id: {}\n",
        cluster.node_id.as_deref().unwrap_or("n/a")
    ));
    out.push_str(&format!(
        "cluster_id: {}\n",
        cluster.cluster_id.as_deref().unwrap_or("n/a")
    ));
    out.push_str(&format!("role: {}\n", cluster.role));
    out.push_str(&format!("term: {}\n", cluster.term));
    out.push_str(&format!(
        "leader_id: {}\n",
        cluster.leader_id.as_deref().unwrap_or("(unknown)")
    ));
    out.push_str(&format!(
        "leader_client_addr: {}\n",
        cluster.leader_client_addr.as_deref().unwrap_or("(unknown)")
    ));
    out.push_str(&format!("single_node: {}\n", cluster.single_node));
    out.push_str(&format!(
        "preview_multi_node: {}\n",
        cluster.preview_multi_node
    ));
    out.push_str(&format!("quorum_available: {}\n", cluster.quorum_available));
    out.push_str(&format!("commit_index: {}\n", cluster.commit_index));
    out.push_str(&format!("applied_index: {}\n", cluster.applied_index));
    out.push_str(&format!("last_log_index: {}\n", cluster.last_log_index));
    out.push_str(&format!(
        "replication_lag_entries: {}\n",
        cluster.replication_lag_entries
    ));
    out.push_str(&format!("peers: {}\n", cluster.peer_count));
    for p in &cluster.peers {
        out.push_str(&format!(
            "  peer {} @ {}{}: {}{}{}\n",
            p.node_id,
            p.addr,
            p.client_addr
                .as_deref()
                .map(|c| format!(" (client {c})"))
                .unwrap_or_default(),
            if p.connected {
                "connected"
            } else {
                "disconnected"
            },
            format!(", connect_attempts={}", p.connect_attempts)
                + &p.match_index
                    .map(|i| format!(", match_index={i}"))
                    .unwrap_or_default(),
            p.next_index
                .map(|i| format!(", next_index={i}"))
                .unwrap_or_default(),
        ));
    }
    if cluster.preview_multi_node {
        out.push_str(
            "note: multi-node mode is an experimental, opt-in preview; single-node mode \
             remains the recommended production mode\n",
        );
    }
    Ok(out)
}

/// `auradb cluster leader` — report the leader recognized by a running server.
pub async fn cmd_cluster_leader(
    addr: &str,
    token: Option<String>,
    tls_ca: Option<PathBuf>,
    server_name: &str,
    json: bool,
) -> Result<String> {
    let report = status_report(addr, token, tls_ca, server_name).await?;
    let cluster = report
        .cluster
        .ok_or_else(|| anyhow::anyhow!("server at {addr} is not running in cluster mode"))?;
    if json {
        return serde_json::to_string_pretty(&serde_json::json!({
            "addr": addr,
            "node_id": cluster.node_id,
            "role": cluster.role,
            "term": cluster.term,
            "leader_id": cluster.leader_id,
            "leader_client_addr": cluster.leader_client_addr,
        }))
        .context("serializing leader report");
    }
    match &cluster.leader_id {
        Some(id) => Ok(format!(
            "leader: {id}\nterm: {}\nrole: {}\nleader_client_addr: {}",
            cluster.term,
            cluster.role,
            cluster.leader_client_addr.as_deref().unwrap_or("(unknown)")
        )),
        None => anyhow::bail!("no leader is currently known by the server at {addr}"),
    }
}

/// `auradb cluster wait-leader` — poll until the server reports a leader.
pub async fn cmd_cluster_wait_leader(
    addr: &str,
    timeout_secs: u64,
    token: Option<String>,
    tls_ca: Option<PathBuf>,
    server_name: &str,
    json: bool,
) -> Result<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if let Ok(report) = status_report(addr, token.clone(), tls_ca.clone(), server_name).await {
            if let Some(cluster) = &report.cluster {
                if let Some(id) = &cluster.leader_id {
                    if json {
                        return serde_json::to_string_pretty(&serde_json::json!({
                            "leader_id": id,
                            "term": cluster.term,
                            "role": cluster.role,
                        }))
                        .context("serializing wait-leader report");
                    }
                    return Ok(format!("leader: {id}\nterm: {}", cluster.term));
                }
            } else {
                anyhow::bail!("server at {addr} is not running in cluster mode");
            }
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("timed out after {timeout_secs}s waiting for a leader at {addr}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// `auradb cluster wait-ready` — poll until the server is reachable and ready.
pub async fn cmd_cluster_wait_ready(
    addr: &str,
    timeout_secs: u64,
    token: Option<String>,
    tls_ca: Option<PathBuf>,
    server_name: &str,
    json: bool,
) -> Result<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if let Ok(report) = status_report(addr, token.clone(), tls_ca.clone(), server_name).await {
            if report.ready {
                if json {
                    return serde_json::to_string_pretty(&serde_json::json!({
                        "addr": addr,
                        "ready": true,
                        "version": report.server_version,
                    }))
                    .context("serializing wait-ready report");
                }
                return Ok(format!("ready: true\nversion: {}", report.server_version));
            }
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("timed out after {timeout_secs}s waiting for {addr} to become ready");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// Connect to a server, ping it, fetch its health frame, and assemble a
/// [`StatusReport`].
async fn status_report(
    addr: &str,
    token: Option<String>,
    tls_ca: Option<PathBuf>,
    server_name: &str,
) -> Result<StatusReport> {
    use auradb_conformance::{ClientTls, ConnectOptions};
    let tls = tls_ca.is_some();
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
    Ok(StatusReport {
        addr: addr.to_string(),
        reachable: true,
        status: format!("{:?}", health.status).to_lowercase(),
        ready: health.ready,
        server_version: health.version,
        protocol_version: PROTOCOL_VERSION,
        collections: health.collections,
        tls,
        mvcc: health.mvcc,
        cluster: health.cluster,
    })
}

/// `auradb cert generate-dev` - generate a self-signed development CA and a
/// server certificate signed by it. The output is suitable for local TLS testing
/// only.
///
/// With no `server_name` and no `sans`, this preserves the original behavior: a
/// `localhost` certificate with SANs `localhost` and `127.0.0.1`, written as
/// `server.crt` / `server.key`. When `server_name` is given, the certificate's
/// Common Name is set to it and the files are named `<server_name>.crt` /
/// `<server_name>.key`, so several nodes can share one CA in one directory. When
/// `sans` is non-empty it sets the Subject Alternative Names verbatim; otherwise
/// the SANs default to the server name plus `localhost` and `127.0.0.1`.
pub fn cmd_cert_generate_dev(
    out_dir: &Path,
    server_name: Option<String>,
    sans: Vec<String>,
) -> Result<String> {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
        IsCa, KeyPair, KeyUsagePurpose,
    };

    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let common_name = server_name
        .clone()
        .unwrap_or_else(|| "localhost".to_string());
    // SANs: explicit list wins; otherwise default to the server name plus the
    // loopback names so a generated cert always validates for local use.
    let mut san_list: Vec<String> = if sans.is_empty() {
        let mut s = vec![common_name.clone()];
        for d in ["localhost", "127.0.0.1"] {
            if !s.iter().any(|x| x == d) {
                s.push(d.to_string());
            }
        }
        s
    } else {
        sans
    };
    san_list.dedup();

    // Deterministic CA parameters: the same subject and constraints every time,
    // so a CA rebuilt from a reloaded key is a functional equal of the persisted
    // one (same subject DN and public key), and leaf certificates it signs chain
    // to the persisted `ca.crt`.
    let ca_params = || -> Result<CertificateParams> {
        let mut p = CertificateParams::new(Vec::new()).map_err(|e| anyhow::anyhow!("{e}"))?;
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        p.use_authority_key_identifier_extension = true;
        p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "AuraDB Development CA");
        p.distinguished_name = dn;
        Ok(p)
    };

    // Reuse a single CA if one already exists in the directory (so per-node certs
    // share a trust root); otherwise generate a fresh CA.
    let ca_path = out_dir.join("ca.crt");
    let ca_key_path = out_dir.join("ca.key");
    let (ca_cert, ca_key, ca_reused) = if ca_key_path.exists() {
        let ca_key = KeyPair::from_pem(&std::fs::read_to_string(&ca_key_path)?)
            .map_err(|e| anyhow::anyhow!("loading existing CA key: {e}"))?;
        let ca_cert = ca_params()?
            .self_signed(&ca_key)
            .map_err(|e| anyhow::anyhow!("rebuilding existing CA: {e}"))?;
        (ca_cert, ca_key, true)
    } else {
        let ca_key = KeyPair::generate().map_err(|e| anyhow::anyhow!("{e}"))?;
        let ca_cert = ca_params()?
            .self_signed(&ca_key)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        std::fs::write(&ca_path, ca_cert.pem())?;
        std::fs::write(&ca_key_path, ca_key.serialize_pem())?;
        restrict_key_permissions(&ca_key_path);
        (ca_cert, ca_key, false)
    };

    let mut srv_params =
        CertificateParams::new(san_list.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
    srv_params.use_authority_key_identifier_extension = true;
    srv_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    // Both server and client auth: the peer (cluster) transport uses mutual TLS,
    // so a node presents this same certificate as a client certificate when it
    // dials a peer. A server-only EKU is rejected by the peer's client-cert
    // verifier ("does not allow extended key usage for client authentication").
    srv_params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];
    let mut srv_dn = DistinguishedName::new();
    srv_dn.push(DnType::CommonName, common_name.clone());
    srv_params.distinguished_name = srv_dn;
    let srv_key = KeyPair::generate().map_err(|e| anyhow::anyhow!("{e}"))?;
    let srv_cert = srv_params
        .signed_by(&srv_key, &ca_cert, &ca_key)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Per-node file names when a server name is given; otherwise the classic
    // server.crt / server.key.
    let stem = server_name.as_deref().unwrap_or("server");
    let cert_path = out_dir.join(format!("{stem}.crt"));
    let key_path = out_dir.join(format!("{stem}.key"));
    std::fs::write(&cert_path, srv_cert.pem())?;
    std::fs::write(&key_path, srv_key.serialize_pem())?;
    restrict_key_permissions(&key_path);

    Ok(format!(
        "WARNING: self-signed development certificates. Do not use them in production.\n\
         {ca_line}\n\
         wrote:\n  {ca}\n  {ca_key}\n  {cert}\n  {key}\n\
         SANs: {sans}\n\n\
         Enable TLS in the server config:\n  [tls]\n  enabled = true\n  \
         cert_path = \"{cert}\"\n  key_path = \"{key}\"\n\n\
         Point clients at the CA with {ca} (server name: {cn}).",
        ca_line = if ca_reused {
            "reused existing CA in this directory"
        } else {
            "generated a new development CA"
        },
        ca = ca_path.display(),
        ca_key = ca_key_path.display(),
        cert = cert_path.display(),
        key = key_path.display(),
        sans = san_list.join(", "),
        cn = common_name,
    ))
}

#[cfg(unix)]
fn restrict_key_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_key_permissions(_path: &Path) {}

/// One measured benchmark result.
#[derive(Debug, Serialize, Deserialize)]
pub struct BenchMeasurement {
    /// The benchmark name.
    pub name: String,
    /// The unit of [`BenchMeasurement::value`] (`ops_per_sec`, `ns_per_op`, or
    /// `seconds`).
    pub unit: String,
    /// The measured value.
    pub value: f64,
    /// The number of iterations measured.
    pub iterations: usize,
}

/// Machine information recorded alongside a benchmark run. Benchmarks are
/// hardware-dependent; this records the environment so a baseline is only ever
/// compared against itself.
#[derive(Debug, Serialize, Deserialize)]
pub struct MachineInfo {
    /// The target operating system.
    pub os: String,
    /// The target architecture.
    pub arch: String,
    /// Available parallelism (logical CPUs), if known.
    pub cpus: Option<usize>,
}

/// A full benchmark report, suitable for a committed baseline snapshot.
#[derive(Debug, Serialize, Deserialize)]
pub struct BenchReport {
    /// The AuraDB version that produced the report.
    pub auradb_version: String,
    /// The number of records inserted for the run.
    pub records: usize,
    /// The command that produced the report.
    pub command: String,
    /// The source commit, if supplied by the caller.
    pub commit: Option<String>,
    /// Machine information.
    pub machine: MachineInfo,
    /// The measured benchmarks.
    pub measurements: Vec<BenchMeasurement>,
}

/// Time `iters` executions of `op` and return throughput in operations/second.
fn ops_per_sec(iters: usize, mut op: impl FnMut() -> Result<()>) -> Result<f64> {
    let start = Instant::now();
    for _ in 0..iters {
        op()?;
    }
    let elapsed = start.elapsed().as_secs_f64().max(1e-9);
    Ok(iters as f64 / elapsed)
}

/// Run the benchmark suite against a fresh schema in `data_dir`. All values are
/// measured live; nothing is fabricated. The run is hardware-dependent and is
/// meant for detecting regressions against a same-machine baseline.
pub fn run_bench(data_dir: &Path, records: usize, commit: Option<String>) -> Result<BenchReport> {
    use auradb::core::{FieldDef, FieldType, IndexDef, IndexKind};
    use auradb::query::{CompareOp, Filter, VectorSearch};
    use auradb::{storage::StorageOptions, EngineOptions};
    use auradb_protocol::{Frame, Opcode, RequestId};

    const DIM: usize = 8;
    // Disable per-commit fsync so the benchmark measures engine work rather than
    // disk-flush latency; the baseline is a relative regression signal.
    let engine = Engine::open_with(
        data_dir,
        EngineOptions {
            storage: StorageOptions {
                sync_on_commit: false,
            },
            ..EngineOptions::default()
        },
    )?;
    let schema = CollectionSchema::new("Bench")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef {
            name: "name".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: false,
            indexed: true,
        })
        .with_field(FieldDef::new("profile", FieldType::Document))
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: DIM }))
        .with_index(IndexDef {
            path: "profile.bucket".into(),
            kind: IndexKind::DocumentPath,
        })
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        });
    engine.create_schema(schema)?;

    let buckets = 64usize;
    let make_record = |i: usize| -> Document {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("b{i}")));
        f.insert("name".into(), Value::Text(format!("name-{i}")));
        let mut profile = Document::new();
        profile.insert(
            "bucket".into(),
            Value::Text(format!("bucket-{}", i % buckets)),
        );
        f.insert("profile".into(), Value::Object(profile));
        f.insert(
            "body".into(),
            Value::Text(format!("alpha bravo charlie record {i} delta echo")),
        );
        let v: Vec<f32> = (0..DIM).map(|j| ((i + j) % 17) as f32).collect();
        f.insert("embedding".into(), Value::Vector(v));
        f
    };

    // Storage append throughput.
    let mut idx = 0usize;
    let append = ops_per_sec(records, || {
        engine.insert("Bench", make_record(idx))?;
        idx += 1;
        Ok(())
    })?;

    let probes = records.clamp(1, 1_000);
    let mut m = Vec::new();
    m.push(BenchMeasurement {
        name: "storage_append".into(),
        unit: "ops_per_sec".into(),
        value: append,
        iterations: records,
    });

    // Point lookup by primary key.
    let mut p = 0usize;
    let point = ops_per_sec(probes, || {
        let mut q = FindQuery::new("Bench");
        q.filter = Some(Filter::Compare {
            field: "id".into(),
            op: CompareOp::Eq,
            value: Value::Text(format!("b{}", p % records.max(1))),
        });
        engine.find(&q)?;
        p += 1;
        Ok(())
    })?;
    m.push(BenchMeasurement {
        name: "point_lookup".into(),
        unit: "ops_per_sec".into(),
        value: point,
        iterations: probes,
    });

    // Secondary index lookup.
    let mut s = 0usize;
    let secondary = ops_per_sec(probes, || {
        let mut q = FindQuery::new("Bench");
        q.filter = Some(Filter::Compare {
            field: "name".into(),
            op: CompareOp::Eq,
            value: Value::Text(format!("name-{}", s % records.max(1))),
        });
        engine.find(&q)?;
        s += 1;
        Ok(())
    })?;
    m.push(BenchMeasurement {
        name: "secondary_index_lookup".into(),
        unit: "ops_per_sec".into(),
        value: secondary,
        iterations: probes,
    });

    // Document-path index lookup.
    let mut d = 0usize;
    let docpath = ops_per_sec(probes, || {
        let mut q = FindQuery::new("Bench");
        q.filter = Some(Filter::Compare {
            field: "profile.bucket".into(),
            op: CompareOp::Eq,
            value: Value::Text(format!("bucket-{}", d % buckets)),
        });
        engine.find(&q)?;
        d += 1;
        Ok(())
    })?;
    m.push(BenchMeasurement {
        name: "document_path_index_lookup".into(),
        unit: "ops_per_sec".into(),
        value: docpath,
        iterations: probes,
    });

    // Full-text lookup.
    let fulltext = ops_per_sec(probes, || {
        let mut q = FindQuery::new("Bench");
        q.filter = Some(Filter::ContainsText {
            field: "body".into(),
            query: "alpha delta".into(),
        });
        q.limit = Some(10);
        engine.find(&q)?;
        Ok(())
    })?;
    m.push(BenchMeasurement {
        name: "full_text_lookup".into(),
        unit: "ops_per_sec".into(),
        value: fulltext,
        iterations: probes,
    });

    // Vector exact nearest neighbour.
    let vector = ops_per_sec(probes.clamp(1, 200), || {
        let mut q = FindQuery::new("Bench");
        q.vector = Some(VectorSearch {
            field: "embedding".into(),
            query: vec![1.0; DIM],
            k: 10,
            metric: "cosine".into(),
        });
        engine.find(&q)?;
        Ok(())
    })?;
    m.push(BenchMeasurement {
        name: "vector_exact_nearest".into(),
        unit: "ops_per_sec".into(),
        value: vector,
        iterations: probes.clamp(1, 200),
    });

    // Cursor paging: walk the collection in pages.
    let page = 100usize;
    let pages = records.div_ceil(page).max(1);
    let paging = ops_per_sec(pages, {
        let mut off = 0usize;
        move || {
            let mut q = FindQuery::new("Bench");
            q.limit = Some(page);
            q.offset = Some(off);
            q.order_by = vec![auradb::query::OrderKey {
                field: "id".into(),
                desc: false,
            }];
            engine.find(&q)?;
            off += page;
            Ok(())
        }
    })?;
    m.push(BenchMeasurement {
        name: "cursor_paging".into(),
        unit: "ops_per_sec".into(),
        value: paging,
        iterations: pages,
    });

    // Frame encode/decode round trip.
    let frame_iters = 100_000usize;
    let frame = Frame::new(Opcode::Ping, RequestId(1), 0, b"ping".to_vec());
    let f_start = Instant::now();
    for _ in 0..frame_iters {
        let bytes = frame.encode();
        let _ = Frame::decode(&bytes, auradb_protocol::DEFAULT_MAX_PAYLOAD)?;
    }
    let frame_ns = f_start.elapsed().as_nanos() as f64 / frame_iters as f64;
    m.push(BenchMeasurement {
        name: "frame_encode_decode".into(),
        unit: "ns_per_op".into(),
        value: frame_ns,
        iterations: frame_iters,
    });

    // Dump and restore round trip.
    let tmp = tempdir_in_parent(data_dir)?;
    let dump_path = tmp.join("bench-dump.jsonl");
    let restore_dir = tmp.join("bench-restore");
    let dr_start = Instant::now();
    cmd_dump(data_dir, &dump_path)?;
    cmd_restore(&restore_dir, &dump_path)?;
    let dr_secs = dr_start.elapsed().as_secs_f64();
    let _ = std::fs::remove_dir_all(&tmp);
    m.push(BenchMeasurement {
        name: "dump_restore".into(),
        unit: "seconds".into(),
        value: dr_secs,
        iterations: 1,
    });

    Ok(BenchReport {
        auradb_version: VERSION.to_string(),
        records,
        command: format!("auradb bench --records {records}"),
        commit,
        machine: MachineInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpus: std::thread::available_parallelism().ok().map(|n| n.get()),
        },
        measurements: m,
    })
}

/// Create a unique scratch directory beside `base` for transient benchmark
/// artifacts. The name combines the process id with a per-call atomic counter so
/// concurrent benchmark runs in the same process (for example parallel tests
/// sharing the system temp dir) never collide on the same scratch path.
fn tempdir_in_parent(base: &Path) -> Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let parent = base.parent().unwrap_or_else(|| Path::new("."));
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = parent.join(format!(".auradb-bench-tmp-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// `auradb bench` - run the local benchmark suite and render a text summary.
pub fn cmd_bench(data_dir: &Path, records: usize) -> Result<String> {
    let report = run_bench(data_dir, records, None)?;
    let mut out = format!("bench results ({records} records):\n");
    for meas in &report.measurements {
        match meas.unit.as_str() {
            "ops_per_sec" => out.push_str(&format!("  {}: {:.0} ops/s\n", meas.name, meas.value)),
            "ns_per_op" => out.push_str(&format!("  {}: {:.1} ns/op\n", meas.name, meas.value)),
            _ => out.push_str(&format!("  {}: {:.4} s\n", meas.name, meas.value)),
        }
    }
    Ok(out.trim_end().to_string())
}

/// `auradb bench --json` - run the suite and return the report as JSON. When
/// `out` is set, the JSON is also written to that path.
pub fn cmd_bench_json(
    data_dir: &Path,
    records: usize,
    commit: Option<String>,
    out: Option<&Path>,
) -> Result<String> {
    let report = run_bench(data_dir, records, commit)?;
    let json = serde_json::to_string_pretty(&report).context("serializing bench report")?;
    if let Some(path) = out {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(path, format!("{json}\n"))
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(json)
}

/// Whether higher values of a unit are better (throughput) or worse (latency,
/// wall time). Determines the sign of a regression for that unit.
fn higher_is_better(unit: &str) -> bool {
    matches!(unit, "ops_per_sec")
}

/// Compare two benchmark baseline reports and render a per-benchmark summary.
///
/// Returns `(report, regressed)`. A benchmark "regresses" when it moves in the
/// worse direction (slower throughput, or higher latency / wall time). When
/// `fail_threshold_percent` is set, `regressed` is true if any benchmark
/// regresses by more than that percentage; otherwise `regressed` is always
/// false (the default — large regressions are reported as warnings but do not
/// fail). Benchmarks are hardware-sensitive: only compare reports produced on
/// the same machine.
pub fn cmd_bench_compare(
    baseline: &Path,
    current: &Path,
    fail_threshold_percent: Option<f64>,
) -> Result<(String, bool)> {
    let base: BenchReport = serde_json::from_str(
        &std::fs::read_to_string(baseline)
            .with_context(|| format!("reading baseline {}", baseline.display()))?,
    )
    .with_context(|| format!("parsing baseline {}", baseline.display()))?;
    let cur: BenchReport = serde_json::from_str(
        &std::fs::read_to_string(current)
            .with_context(|| format!("reading current {}", current.display()))?,
    )
    .with_context(|| format!("parsing current {}", current.display()))?;

    let base_by_name: std::collections::HashMap<&str, &BenchMeasurement> = base
        .measurements
        .iter()
        .map(|m| (m.name.as_str(), m))
        .collect();

    let mut out = format!(
        "benchmark comparison: {} ({}) -> {} ({})\n",
        base.auradb_version,
        base.commit.as_deref().unwrap_or("?"),
        cur.auradb_version,
        cur.commit.as_deref().unwrap_or("?"),
    );
    if base.machine.os != cur.machine.os || base.machine.arch != cur.machine.arch {
        out.push_str(
            "warning: reports were produced on different machines; comparison is unreliable\n",
        );
    }
    out.push_str("hardware-sensitive: compare only reports from the same machine.\n");

    let mut worst_regression = 0.0_f64;
    for m in &cur.measurements {
        let Some(b) = base_by_name.get(m.name.as_str()) else {
            out.push_str(&format!("  {}: new (no baseline)\n", m.name));
            continue;
        };
        // Percent change in the value, then translate into a "better/worse"
        // delta accounting for the metric direction.
        let pct = if b.value.abs() < f64::EPSILON {
            0.0
        } else {
            (m.value - b.value) / b.value * 100.0
        };
        let regression_pct = if higher_is_better(&m.unit) { -pct } else { pct };
        let marker = if regression_pct > 0.0 {
            " REGRESSION"
        } else {
            ""
        };
        if regression_pct > worst_regression {
            worst_regression = regression_pct;
        }
        out.push_str(&format!(
            "  {}: {:.2} -> {:.2} {} ({:+.1}%){}\n",
            m.name, b.value, m.value, m.unit, pct, marker
        ));
    }

    let regressed = match fail_threshold_percent {
        Some(threshold) => {
            let fail = worst_regression > threshold;
            out.push_str(&format!(
                "worst regression: {:.1}% (fail threshold {:.1}%) -> {}",
                worst_regression,
                threshold,
                if fail { "FAIL" } else { "ok" }
            ));
            fail
        }
        None => {
            out.push_str(&format!(
                "worst regression: {:.1}% (no fail threshold set; warnings only)",
                worst_regression
            ));
            false
        }
    };
    Ok((out, regressed))
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
    fn snapshot_create_inspect_restore_roundtrips() {
        use auradb::core::{FieldDef, FieldType};
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        {
            let engine = Engine::open(&data).unwrap();
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
        let snap = dir.path().join("snap.aura");
        let out = cmd_snapshot_create(&data, &snap).unwrap();
        assert!(out.contains("3 record(s)"), "{out}");

        let inspect = cmd_snapshot_inspect(&snap).unwrap();
        assert!(inspect.contains("records: 3"), "{inspect}");
        assert!(inspect.contains("integrity: ok"), "{inspect}");

        let restore_dir = dir.path().join("restored");
        let r = cmd_snapshot_restore(&snap, &restore_dir, false).unwrap();
        assert!(r.contains("3 record(s)"), "{r}");
        let engine = Engine::open(&restore_dir).unwrap();
        assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 3);
    }

    #[test]
    fn snapshot_restore_refuses_nonempty_without_force() {
        use auradb::core::{FieldDef, FieldType};
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        {
            let engine = Engine::open(&data).unwrap();
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
            let mut f = Document::new();
            f.insert("id".into(), Value::Text("r0".into()));
            engine.insert("C", f).unwrap();
        }
        let snap = dir.path().join("snap.aura");
        cmd_snapshot_create(&data, &snap).unwrap();
        let target = dir.path().join("target");
        cmd_snapshot_restore(&snap, &target, false).unwrap();
        // A second restore into the now-populated target is refused without force.
        assert!(cmd_snapshot_restore(&snap, &target, false).is_err());
        // With force it succeeds.
        assert!(cmd_snapshot_restore(&snap, &target, true).is_ok());
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
    fn gc_and_stats_commands() {
        use auradb::core::{CollectionSchema, FieldDef, FieldType};
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        {
            let engine = Engine::open(&data).unwrap();
            engine
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
            for i in 0..5 {
                let mut f = Document::new();
                f.insert("id".into(), Value::Text(format!("r{i}")));
                f.insert("v".into(), Value::Int(i));
                engine.insert("C", f).unwrap();
            }
        }
        // stats analyze then show reflect the data.
        assert!(cmd_stats_analyze(&data).unwrap().contains("analyzed 1"));
        let show = cmd_stats_show(&data, false).unwrap();
        assert!(show.contains("C: 5 rows"), "{show}");
        let json = cmd_stats_show(&data, true).unwrap();
        assert!(json.contains("\"row_count\": 5"), "{json}");
        // gc runs cleanly, and a dry run reports without modifying data.
        let dry = cmd_gc(&data, true, false).unwrap();
        assert!(dry.contains("dry run"), "{dry}");
        assert!(cmd_gc(&data, false, false)
            .unwrap()
            .contains("garbage collection"));
        let json = cmd_gc(&data, false, true).unwrap();
        assert!(json.contains("\"versions_reclaimed\""), "{json}");
    }

    fn stats_with(
        active: usize,
        oldest_age: Option<u64>,
        records: usize,
        versions: usize,
    ) -> auradb::EngineStats {
        auradb::EngineStats {
            collections: 1,
            records,
            versions,
            active_transactions: active,
            timed_out_transactions: 0,
            oldest_active_read_ts: oldest_age.map(|_| 1),
            oldest_transaction_age_secs: oldest_age,
            transaction_timeouts_total: 0,
            schema_version: 1,
        }
    }

    #[test]
    fn doctor_warns_when_gc_disabled() {
        let mut config = Config::default();
        config.mvcc.gc_enabled = false;
        let (mvcc, warnings) = mvcc_doctor(&stats_with(0, None, 0, 0), &config, true, false);
        assert!(!mvcc.gc_enabled);
        assert!(
            warnings.iter().any(|w| w.contains("GC is disabled")),
            "{warnings:?}"
        );
    }

    #[test]
    fn doctor_warns_on_long_lived_snapshot() {
        let config = Config::default();
        let (_mvcc, warnings) = mvcc_doctor(&stats_with(1, Some(7200), 1, 1), &config, true, false);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("oldest active snapshot")),
            "{warnings:?}"
        );
    }

    #[test]
    fn doctor_warns_on_version_pressure_and_stale_stats() {
        let config = Config::default();
        let (_mvcc, warnings) = mvcc_doctor(&stats_with(0, None, 10, 1000), &config, true, true);
        assert!(warnings.iter().any(|w| w.contains("retained versions")));
        assert!(warnings.iter().any(|w| w.contains("stale")));
    }

    #[test]
    fn secrets_still_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        cmd_init(&data, &dir.path().join("AuraDB.toml")).unwrap();
        let hash = auradb_server::auth::hash_token("super-secret-token").unwrap();
        let mut config = Config {
            data_dir: data.clone(),
            ..Config::default()
        };
        config.auth.enabled = true;
        config.auth.token_hash = Some(hash.clone());
        let json = cmd_doctor_json(&data, &config).unwrap();
        assert!(!json.contains(&hash), "doctor JSON leaked the token hash");
        assert!(json.contains("auth_token_hash_configured"));
        let text = cmd_doctor(&data, &config).unwrap();
        assert!(!text.contains(&hash), "doctor text leaked the token hash");
    }

    #[test]
    fn bench_compare_flags_throughput_regression() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.json");
        let cur = dir.path().join("cur.json");
        let report = |insert: f64| {
            format!(
                r#"{{"auradb_version":"0.3.1","records":10,"command":"bench","commit":null,
                "machine":{{"os":"{os}","arch":"{arch}","cpus":null}},
                "measurements":[{{"name":"insert","unit":"ops_per_sec","value":{insert},"iterations":10}}]}}"#,
                os = std::env::consts::OS,
                arch = std::env::consts::ARCH,
            )
        };
        std::fs::write(&base, report(1000.0)).unwrap();
        std::fs::write(&cur, report(500.0)).unwrap(); // 50% slower throughput
        let (out, regressed) = cmd_bench_compare(&base, &cur, None).unwrap();
        assert!(out.contains("REGRESSION"), "{out}");
        assert!(!regressed, "no fail threshold set -> warnings only");
        let (_out, regressed) = cmd_bench_compare(&base, &cur, Some(10.0)).unwrap();
        assert!(regressed, "50% regression exceeds the 10% fail threshold");
        // An improvement is never a regression.
        std::fs::write(&cur, report(2000.0)).unwrap();
        let (_out, regressed) = cmd_bench_compare(&base, &cur, Some(10.0)).unwrap();
        assert!(!regressed);
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
        assert!(cmd_config_validate(&cfg, false).unwrap().contains("valid"));

        let bad = dir.path().join("bad.toml");
        std::fs::write(&bad, "bind = \"0.0.0.0\"\nport = 7171\n").unwrap();
        assert!(cmd_config_validate(&bad, false).is_err());
    }

    #[test]
    fn config_validate_structural_accepts_secure_template_without_certs() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("secure.toml");
        // Auth enabled with a real hash, TLS enabled pointing at paths that do
        // not exist on this host.
        let hash = auradb_server::auth::hash_token("secret").unwrap();
        std::fs::write(
            &cfg,
            format!(
                "bind = \"0.0.0.0\"\nport = 7171\n\
                 [auth]\nenabled = true\ntoken_hash = \"{hash}\"\n\
                 [tls]\nenabled = true\ncert_path = \"/does/not/exist/server.crt\"\n\
                 key_path = \"/does/not/exist/server.key\"\n"
            ),
        )
        .unwrap();
        // Full validation fails because the cert files are missing.
        assert!(cmd_config_validate(&cfg, false).is_err());
        // Structural validation passes (files live on the target host).
        assert!(cmd_config_validate(&cfg, true).unwrap().contains("valid"));

        // Structural validation still rejects a genuinely invalid secure config
        // (auth enabled without a token hash).
        let bad = dir.path().join("bad-secure.toml");
        std::fs::write(
            &bad,
            "bind = \"0.0.0.0\"\nport = 7171\n[auth]\nenabled = true\n",
        )
        .unwrap();
        assert!(cmd_config_validate(&bad, true).is_err());
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
    fn rotate_token_updates_hash_and_redacts() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let cfg = dir.path().join("AuraDB.toml");
        cmd_init(&data, &cfg).unwrap();

        let out = cmd_auth_rotate_token(&cfg, Some("new-secret".into()), true).unwrap();
        // The plaintext token is never echoed.
        assert!(!out.contains("new-secret"));
        assert!(out.contains("redacted"));

        // The written config verifies the new token and not the old one.
        let written = Config::load(&cfg).unwrap();
        assert!(written.auth.enabled);
        let hash = written.auth.token_hash.expect("token hash written");
        assert!(hash.starts_with("$argon2id$"));
        assert!(!hash.contains("new-secret"));
        assert!(auradb_server::auth::verify_token(&hash, "new-secret").unwrap());
        assert!(!auradb_server::auth::verify_token(&hash, "old-secret").unwrap());

        // A backup of the previous config exists.
        assert!(backup_path_for(&cfg).exists());

        // The on-disk file must not contain the plaintext token anywhere.
        let raw = std::fs::read_to_string(&cfg).unwrap();
        assert!(!raw.contains("new-secret"));
    }

    #[test]
    fn rotate_token_preserves_unrelated_fields() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let cfg = dir.path().join("AuraDB.toml");
        cmd_init(&data, &cfg).unwrap();
        // Change an unrelated field, then rotate.
        let mut c = Config::load(&cfg).unwrap();
        c.port = 9999;
        c.cursor_timeout_secs = 123;
        std::fs::write(&cfg, c.to_toml()).unwrap();

        cmd_auth_rotate_token(&cfg, Some("tok".into()), false).unwrap();
        let after = Config::load(&cfg).unwrap();
        assert_eq!(after.port, 9999);
        assert_eq!(after.cursor_timeout_secs, 123);
    }

    #[test]
    fn rotate_token_rejects_empty_and_missing_config() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("AuraDB.toml");
        cmd_init(&dir.path().join("data"), &cfg).unwrap();
        assert!(cmd_auth_rotate_token(&cfg, Some(String::new()), false).is_err());
        let missing = dir.path().join("nope.toml");
        assert!(cmd_auth_rotate_token(&missing, Some("t".into()), false).is_err());
    }

    #[test]
    fn doctor_json_parses_and_redacts() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        cmd_init(&data, &dir.path().join("c.toml")).unwrap();
        let hash = auradb_server::auth::hash_token("super-secret").unwrap();
        let config = Config {
            data_dir: data.clone(),
            auth: auradb_server::AuthConfig {
                enabled: true,
                token_hash: Some(hash.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let json = cmd_doctor_json(&data, &config).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["security"]["auth_enabled"], true);
        assert_eq!(v["security"]["auth_token_hash_configured"], true);
        assert_eq!(v["index_consistency_ok"], true);
        assert!(!json.contains(&hash));
        assert!(!json.contains("super-secret"));
    }

    #[test]
    fn bench_json_measures_categories() {
        let dir = tempfile::tempdir().unwrap();
        let json = cmd_bench_json(dir.path(), 200, Some("abc1234".into()), None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["commit"], "abc1234");
        let names: Vec<String> = v["measurements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap().to_string())
            .collect();
        for expected in [
            "storage_append",
            "point_lookup",
            "secondary_index_lookup",
            "document_path_index_lookup",
            "full_text_lookup",
            "vector_exact_nearest",
            "cursor_paging",
            "frame_encode_decode",
            "dump_restore",
        ] {
            assert!(names.iter().any(|n| n == expected), "missing {expected}");
        }
    }

    #[test]
    fn cert_generate_dev_creates_usable_files() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("certs");
        let report = cmd_cert_generate_dev(&out, None, Vec::new()).unwrap();
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

    #[test]
    fn cert_generate_dev_per_node_shares_one_ca() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("certs");
        // Generate three per-node certs with explicit SANs into one directory.
        for node in ["node1", "node2", "node3"] {
            let report = cmd_cert_generate_dev(
                &out,
                Some(node.to_string()),
                vec![node.to_string(), "localhost".into(), "127.0.0.1".into()],
            )
            .unwrap();
            assert!(report.contains("node1") || report.contains(node));
            assert!(
                out.join(format!("{node}.crt")).exists(),
                "{node}.crt should be written"
            );
            assert!(out.join(format!("{node}.key")).exists());
        }
        // Exactly one CA is shared across the nodes.
        assert!(out.join("ca.crt").exists());
        let ca_after_first = std::fs::read_to_string(out.join("ca.crt")).unwrap();
        // Each per-node certificate loads into the server's TLS stack.
        for node in ["node1", "node2", "node3"] {
            let scfg = auradb_server::Config {
                data_dir: dir.path().join(format!("data-{node}")),
                tls: auradb_server::TlsConfig {
                    enabled: true,
                    cert_path: Some(out.join(format!("{node}.crt"))),
                    key_path: Some(out.join(format!("{node}.key"))),
                    ..Default::default()
                },
                ..Default::default()
            };
            auradb_server::Server::open(scfg).expect("generated per-node cert should load");
        }
        // The CA was reused, not regenerated, by later nodes.
        assert_eq!(
            ca_after_first,
            std::fs::read_to_string(out.join("ca.crt")).unwrap()
        );
    }
}
