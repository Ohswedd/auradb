//! `auradb cluster ...` commands: identity, status, peers, doctor, bootstrap.
//!
//! These commands operate on a data directory's cluster metadata and the parsed
//! `[cluster]` configuration. They never expose membership operations
//! (`join` / `leave` / `step-down`) because membership changes are not
//! implemented in this release — surfacing them would be a placeholder.

use std::path::Path;

use anyhow::{Context, Result};
use auradb::Engine;
use auradb_cluster::{ClusterConfig, ClusterStore, NodeId};
use auradb_server::Config;
use serde::Serialize;

use crate::VERSION;

/// Offline view of a node's cluster metadata and configuration.
#[derive(Debug, Serialize)]
pub struct ClusterMetadataReport {
    /// Whether `[cluster]` is enabled in the configuration.
    pub enabled: bool,
    /// Whether cluster identity has been initialized on disk.
    pub initialized: bool,
    /// This node's id (hex), if initialized.
    pub node_id: Option<String>,
    /// The cluster id (hex), if initialized.
    pub cluster_id: Option<String>,
    /// Whether this is a single-node cluster (no peers configured).
    pub single_node: bool,
    /// Number of configured peers.
    pub peer_count: usize,
    /// The configured peers.
    pub peers: Vec<String>,
    /// The cluster listen address.
    pub listen_addr: String,
    /// The advertised cluster address.
    pub advertise_addr: String,
    /// Whether this node bootstraps a new cluster.
    pub bootstrap: bool,
    /// How this view was produced (always local metadata for offline commands).
    pub source: String,
}

fn pinned(value: &str) -> Result<Option<NodeId>> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(value.trim().parse::<NodeId>().map_err(|e| {
        anyhow::anyhow!("invalid cluster.node_id: {e}")
    })?))
}

/// Build the offline cluster metadata report for a data directory. Loading the
/// metadata validates its on-disk format, so a future format version is rejected
/// here (this is how `auradb doctor` validates cluster metadata).
pub fn cluster_metadata_report(data_dir: &Path, config: &Config) -> Result<ClusterMetadataReport> {
    metadata_report(data_dir, config)
}

fn metadata_report(data_dir: &Path, config: &Config) -> Result<ClusterMetadataReport> {
    let store = ClusterStore::new(data_dir);
    let identity = store.load().context("loading cluster metadata")?;
    let cluster = &config.cluster;
    Ok(ClusterMetadataReport {
        enabled: cluster.enabled,
        initialized: identity.is_some(),
        node_id: identity.as_ref().map(|i| i.node_id().to_string()),
        cluster_id: identity.as_ref().map(|i| i.cluster_id().to_string()),
        single_node: cluster.peers.is_empty(),
        peer_count: cluster.peers.len(),
        peers: cluster
            .peers
            .iter()
            .map(|p| format!("{}@{}", p.node_id, p.addr))
            .collect(),
        listen_addr: cluster.listen_addr.clone(),
        advertise_addr: cluster.advertise_addr.clone(),
        bootstrap: cluster.bootstrap,
        source: "local-metadata".into(),
    })
}

/// `auradb cluster init` — create stable node + cluster identity if absent.
pub fn cmd_cluster_init(data_dir: &Path, config: &Config) -> Result<String> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating data dir {}", data_dir.display()))?;
    Engine::open(data_dir).context("initializing storage")?;
    let store = ClusterStore::new(data_dir);
    let identity = store
        .init(pinned(&config.cluster.node_id)?, None, VERSION)
        .context("initializing cluster identity")?;
    // Re-honor a pinned cluster id by validating it matches (init handles conflict).
    let mut out = String::new();
    out.push_str("cluster identity initialized\n");
    out.push_str(&format!("node_id: {}\n", identity.node_id()));
    out.push_str(&format!("cluster_id: {}\n", identity.cluster_id()));
    out.push_str(&format!("data_dir: {}\n", data_dir.display()));
    Ok(out)
}

/// `auradb cluster bootstrap` — form a durable single-node cluster and elect
/// this node leader. Fails closed if the configuration describes a multi-node
/// deployment (which is not supported in this release).
pub fn cmd_cluster_bootstrap(data_dir: &Path, config: &Config) -> Result<String> {
    if config.cluster.is_multi_node() {
        anyhow::bail!(
            "`auradb cluster bootstrap` forms a durable single-node cluster; it does not start \
             the multi-node preview. Remove peers to bootstrap a single-node cluster, or start a \
             multi-node preview node with `auradb server --config <node>.toml`"
        );
    }
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating data dir {}", data_dir.display()))?;
    let engine = Engine::open(data_dir).context("opening engine")?;
    let store = ClusterStore::new(data_dir);
    let identity = store
        .init(pinned(&config.cluster.node_id)?, None, VERSION)
        .context("initializing cluster identity")?;
    // Bootstrapping always forms a single-node cluster.
    let mut cluster_cfg = ClusterConfig::single_node();
    cluster_cfg.node_id = identity.node_id().to_string();
    let node =
        auradb_replication::ClusterNode::bootstrap(engine, identity, cluster_cfg, store.dir())
            .context("bootstrapping single-node cluster")?;
    let status = node.status();
    let mut out = String::new();
    out.push_str("single-node cluster bootstrapped\n");
    out.push_str(&format!("node_id: {}\n", status.node_id.unwrap()));
    out.push_str(&format!("cluster_id: {}\n", status.cluster_id.unwrap()));
    out.push_str(&format!("role: {}\n", status.role));
    out.push_str(&format!("term: {}\n", status.term));
    out.push_str(&format!("commit_index: {}\n", status.commit_index));
    Ok(out)
}

/// `auradb cluster status` — show local cluster metadata.
pub fn cmd_cluster_status(data_dir: &Path, config: &Config, json: bool) -> Result<String> {
    let report = metadata_report(data_dir, config)?;
    if json {
        return Ok(serde_json::to_string_pretty(&report)?);
    }
    let mut out = String::new();
    out.push_str(&format!("cluster_enabled: {}\n", report.enabled));
    out.push_str(&format!("initialized: {}\n", report.initialized));
    out.push_str(&format!(
        "node_id: {}\n",
        report.node_id.as_deref().unwrap_or("(uninitialized)")
    ));
    out.push_str(&format!(
        "cluster_id: {}\n",
        report.cluster_id.as_deref().unwrap_or("(uninitialized)")
    ));
    out.push_str(&format!("single_node: {}\n", report.single_node));
    out.push_str(&format!("peers: {}\n", report.peer_count));
    out.push_str(&format!("listen_addr: {}\n", report.listen_addr));
    out.push_str("note: runtime role/term are reported by `auradb status --addr <server>`\n");
    Ok(out)
}

/// `auradb cluster peers` — list configured peers.
pub fn cmd_cluster_peers(data_dir: &Path, config: &Config, json: bool) -> Result<String> {
    let report = metadata_report(data_dir, config)?;
    if json {
        return Ok(serde_json::to_string_pretty(&report.peers)?);
    }
    if report.peers.is_empty() {
        return Ok("no peers configured (single-node cluster)\n".to_string());
    }
    let mut out = String::new();
    for peer in &report.peers {
        out.push_str(&format!("{peer}\n"));
    }
    Ok(out)
}

/// The result of `auradb cluster doctor`.
#[derive(Debug, Serialize)]
pub struct ClusterDoctorReport {
    /// Whether the cluster configuration is valid.
    pub config_valid: bool,
    /// The metadata view.
    pub metadata: ClusterMetadataReport,
    /// Whether this node looks healthy for a single-node cluster.
    pub healthy: bool,
    /// Operational warnings.
    pub warnings: Vec<String>,
}

/// `auradb cluster doctor` — validate cluster config and metadata.
pub fn cmd_cluster_doctor(data_dir: &Path, config: &Config, json: bool) -> Result<String> {
    // Configuration validation fails closed: an invalid cluster config is an
    // error, not a warning.
    config.validate().context("config validation")?;
    let metadata = metadata_report(data_dir, config)?;

    let mut warnings = Vec::new();
    if config.cluster.enabled {
        if !metadata.initialized {
            warnings.push(
                "cluster mode is enabled but no identity is initialized; run `auradb cluster init`"
                    .to_string(),
            );
        }
        if config.cluster.peers.is_empty() && !config.cluster.bootstrap {
            warnings.push(
                "cluster mode is enabled with no peers and bootstrap = false; this node cannot \
                 form or join a cluster"
                    .to_string(),
            );
        }
        if !config.cluster.is_loopback() && !config.allow_insecure_bind {
            warnings.push(format!(
                "cluster listen_addr {} is not loopback and cluster transport is unauthenticated \
                 in this release",
                config.cluster.listen_addr
            ));
        }
        if config.cluster.experimental_multi_node {
            warnings.push(
                "multi-node mode is an experimental, opt-in preview; single-node mode remains the \
                 recommended production mode. Runtime leader, quorum, and per-peer state are \
                 reported by `auradb cluster status --addr <server>`"
                    .to_string(),
            );
        }
        if config.cluster.is_public() && !config.cluster.tls.enabled {
            warnings.push(
                "public cluster transport without peer TLS is rejected at startup; configure \
                 [cluster.tls] (cert_path, key_path, ca_path) and a peer_auth_token before \
                 exposing the cluster beyond loopback"
                    .to_string(),
            );
        }
        if config.cluster.is_public() && config.cluster.peer_auth_token.is_empty() {
            warnings.push(
                "public cluster transport without a peer authentication token is rejected at \
                 startup; set [cluster] peer_auth_token"
                    .to_string(),
            );
        }
    }

    let healthy = config.cluster.enabled && metadata.initialized && warnings.is_empty();
    let report = ClusterDoctorReport {
        config_valid: true,
        metadata,
        healthy,
        warnings,
    };

    if json {
        return Ok(serde_json::to_string_pretty(&report)?);
    }
    let mut out = String::new();
    out.push_str(&format!("config_valid: {}\n", report.config_valid));
    out.push_str(&format!("cluster_enabled: {}\n", report.metadata.enabled));
    out.push_str(&format!("initialized: {}\n", report.metadata.initialized));
    out.push_str(&format!(
        "single_node_healthy: {}\n",
        report.healthy && report.metadata.single_node
    ));
    if report.warnings.is_empty() {
        out.push_str("warnings: none\n");
    } else {
        for w in &report.warnings {
            out.push_str(&format!("warning: {w}\n"));
        }
    }
    Ok(out)
}

/// `auradb cluster compact-log` — compact the durable Raft log up to the safely
/// applied prefix. With `dry_run`, report what would be discarded without
/// modifying anything. Fails closed on a multi-node configuration (not enabled in
/// this release) and on an uninitialized cluster.
pub fn cmd_cluster_compact_log(
    data_dir: &Path,
    config: &Config,
    dry_run: bool,
    json: bool,
) -> Result<String> {
    if config.cluster.is_multi_node() {
        anyhow::bail!(
            "multi-node cluster deployment is experimental and not enabled in this release; \
             remove peers to operate a single-node cluster"
        );
    }
    let store = ClusterStore::new(data_dir);
    let identity = store
        .load()
        .context("loading cluster metadata")?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cluster identity is not initialized; run `auradb cluster bootstrap` first"
            )
        })?;
    let engine = Engine::open(data_dir).context("opening engine")?;
    let mut cluster_cfg = ClusterConfig::single_node();
    cluster_cfg.node_id = identity.node_id().to_string();
    let node =
        auradb_replication::ClusterNode::bootstrap(engine, identity, cluster_cfg, store.dir())
            .context("opening single-node cluster")?;
    let report = node
        .compact_log(dry_run)
        .context("compacting the raft log")?;
    if json {
        return Ok(serde_json::to_string_pretty(&report)?);
    }
    let mut out = String::new();
    if report.dry_run {
        out.push_str("dry run: no data modified\n");
    }
    if report.compacted {
        out.push_str(&format!(
            "{} entr{} {} compacted\n",
            report.entries_discarded,
            if report.entries_discarded == 1 {
                "y"
            } else {
                "ies"
            },
            if report.dry_run { "would be" } else { "were" },
        ));
    } else {
        out.push_str("nothing to compact (no applied prefix beyond the current boundary)\n");
    }
    out.push_str(&format!(
        "last_included_index: {}\nlast_included_term: {}\ncommit_index: {}\napplied_index: {}\nlast_log_index: {}\n",
        report.last_included_index,
        report.last_included_term,
        report.commit_index,
        report.applied_index,
        report.last_log_index,
    ));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn cluster_config(data_dir: &Path) -> Config {
        Config {
            data_dir: data_dir.to_path_buf(),
            cluster: ClusterConfig::single_node(),
            ..Config::default()
        }
    }

    #[test]
    fn cluster_init_creates_metadata() {
        let dir = tempdir().unwrap();
        let cfg = cluster_config(dir.path());
        let out = cmd_cluster_init(dir.path(), &cfg).unwrap();
        assert!(out.contains("node_id:"));
        assert!(ClusterStore::new(dir.path()).is_initialized());
    }

    #[test]
    fn cluster_status_reports_node() {
        let dir = tempdir().unwrap();
        let cfg = cluster_config(dir.path());
        cmd_cluster_init(dir.path(), &cfg).unwrap();
        let json = cmd_cluster_status(dir.path(), &cfg, true).unwrap();
        assert!(json.contains("\"initialized\": true"));
        assert!(json.contains("\"single_node\": true"));
    }

    #[test]
    fn cluster_doctor_validates_metadata() {
        let dir = tempdir().unwrap();
        let cfg = cluster_config(dir.path());
        cmd_cluster_init(dir.path(), &cfg).unwrap();
        let out = cmd_cluster_doctor(dir.path(), &cfg, false).unwrap();
        assert!(out.contains("config_valid: true"));
    }

    #[test]
    fn cluster_doctor_reports_single_node_healthy() {
        let dir = tempdir().unwrap();
        let cfg = cluster_config(dir.path());
        cmd_cluster_init(dir.path(), &cfg).unwrap();
        let json = cmd_cluster_doctor(dir.path(), &cfg, true).unwrap();
        assert!(json.contains("\"healthy\": true"));
    }

    #[test]
    fn cluster_bootstrap_single_node() {
        let dir = tempdir().unwrap();
        let cfg = cluster_config(dir.path());
        let out = cmd_cluster_bootstrap(dir.path(), &cfg).unwrap();
        assert!(out.contains("role: leader"));
    }

    #[test]
    fn invalid_cluster_config_fails() {
        let dir = tempdir().unwrap();
        let mut cluster = ClusterConfig::single_node();
        cluster.listen_addr = "not-an-addr".into();
        let cfg = Config {
            data_dir: dir.path().to_path_buf(),
            cluster,
            ..Config::default()
        };
        assert!(cmd_cluster_doctor(dir.path(), &cfg, false).is_err());
    }

    #[test]
    fn cluster_compact_log_dry_run_json() {
        let dir = tempdir().unwrap();
        let cfg = cluster_config(dir.path());
        // Bootstrap so identity + raft log exist.
        cmd_cluster_bootstrap(dir.path(), &cfg).unwrap();
        let json = cmd_cluster_compact_log(dir.path(), &cfg, true, true).unwrap();
        assert!(json.contains("\"dry_run\": true"), "{json}");
        assert!(json.contains("\"last_included_index\""), "{json}");
    }

    #[test]
    fn cluster_compact_log_requires_initialized_cluster() {
        let dir = tempdir().unwrap();
        let cfg = cluster_config(dir.path());
        // No bootstrap/init: compaction must refuse rather than guess.
        assert!(cmd_cluster_compact_log(dir.path(), &cfg, true, false).is_err());
    }

    #[test]
    fn multi_node_bootstrap_fails_closed() {
        let dir = tempdir().unwrap();
        let mut cluster = ClusterConfig::single_node();
        cluster.experimental_multi_node = true;
        cluster.peers = vec![auradb_cluster::PeerConfig {
            node_id: "00000000000000a2".into(),
            addr: "127.0.0.1:7272".into(),
            client_addr: None,
        }];
        let cfg = Config {
            data_dir: dir.path().to_path_buf(),
            cluster,
            ..Config::default()
        };
        assert!(cmd_cluster_bootstrap(dir.path(), &cfg).is_err());
    }
}
