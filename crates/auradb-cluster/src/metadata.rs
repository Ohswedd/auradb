//! Durable cluster identity: node and cluster metadata files.
//!
//! Identity lives under `<data_dir>/cluster/`:
//!
//! ```text
//! cluster/
//!   node.json      # this node's stable id
//!   cluster.json   # the cluster this node belongs to
//! ```
//!
//! Both files carry a `format_version`. A file written by a newer AuraDB (a
//! higher `format_version`) is rejected rather than guessed at — AuraDB fails
//! closed on unknown future formats.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{ClusterError, Result};
use crate::id::{ClusterId, NodeId};

/// The on-disk metadata format version this build writes and understands.
pub const METADATA_FORMAT_VERSION: u32 = 1;

const CLUSTER_DIR: &str = "cluster";
const NODE_FILE: &str = "node.json";
const CLUSTER_FILE: &str = "cluster.json";

/// Persisted identity of a single node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeMetadata {
    /// On-disk format version. Rejected if greater than [`METADATA_FORMAT_VERSION`].
    pub format_version: u32,
    /// This node's stable id.
    pub node_id: NodeId,
    /// The AuraDB version that first initialized this node.
    pub created_by_version: String,
}

/// Persisted identity of the cluster a node belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterMetadata {
    /// On-disk format version. Rejected if greater than [`METADATA_FORMAT_VERSION`].
    pub format_version: u32,
    /// The cluster's stable id.
    pub cluster_id: ClusterId,
    /// The AuraDB version that bootstrapped this cluster.
    pub created_by_version: String,
}

/// A node's combined, validated identity (node + cluster).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterIdentity {
    /// This node's metadata.
    pub node: NodeMetadata,
    /// The cluster metadata.
    pub cluster: ClusterMetadata,
}

impl ClusterIdentity {
    /// This node's id.
    pub fn node_id(&self) -> NodeId {
        self.node.node_id
    }

    /// The cluster id.
    pub fn cluster_id(&self) -> ClusterId {
        self.cluster.cluster_id
    }
}

/// Reads and writes cluster identity under a data directory.
#[derive(Debug, Clone)]
pub struct ClusterStore {
    dir: PathBuf,
}

impl ClusterStore {
    /// Bind a store to `<data_dir>/cluster/`.
    pub fn new(data_dir: impl AsRef<Path>) -> ClusterStore {
        ClusterStore {
            dir: data_dir.as_ref().join(CLUSTER_DIR),
        }
    }

    /// The cluster directory this store manages.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn node_path(&self) -> PathBuf {
        self.dir.join(NODE_FILE)
    }

    fn cluster_path(&self) -> PathBuf {
        self.dir.join(CLUSTER_FILE)
    }

    /// Whether this node has been initialized for cluster mode.
    pub fn is_initialized(&self) -> bool {
        self.node_path().exists() && self.cluster_path().exists()
    }

    /// Load and validate the persisted identity, or `None` if not initialized.
    pub fn load(&self) -> Result<Option<ClusterIdentity>> {
        if !self.is_initialized() {
            // Partial state (one file but not the other) is a hard error: we
            // never silently re-initialize over half-written identity.
            if self.node_path().exists() || self.cluster_path().exists() {
                return Err(ClusterError::IdentityConflict(format!(
                    "incomplete cluster identity under {}: expected both {NODE_FILE} and \
                     {CLUSTER_FILE}",
                    self.dir.display()
                )));
            }
            return Ok(None);
        }
        let node: NodeMetadata = read_json(&self.node_path())?;
        check_format(&self.node_path(), node.format_version)?;
        let cluster: ClusterMetadata = read_json(&self.cluster_path())?;
        check_format(&self.cluster_path(), cluster.format_version)?;
        Ok(Some(ClusterIdentity { node, cluster }))
    }

    /// Initialize identity if absent, honoring any pinned ids, and return it.
    ///
    /// If identity already exists it is loaded and checked against the pinned
    /// ids (if any); a mismatch is a hard [`ClusterError::IdentityConflict`].
    /// Generation of fresh ids happens only when nothing is persisted yet.
    pub fn init(
        &self,
        pinned_node: Option<NodeId>,
        pinned_cluster: Option<ClusterId>,
        version: &str,
    ) -> Result<ClusterIdentity> {
        if let Some(existing) = self.load()? {
            if let Some(want) = pinned_node {
                if existing.node.node_id != want {
                    return Err(ClusterError::IdentityConflict(format!(
                        "configured node_id {want} does not match persisted node_id {}",
                        existing.node.node_id
                    )));
                }
            }
            if let Some(want) = pinned_cluster {
                if existing.cluster.cluster_id != want {
                    return Err(ClusterError::IdentityConflict(format!(
                        "configured cluster_id {want} does not match persisted cluster_id {}",
                        existing.cluster.cluster_id
                    )));
                }
            }
            return Ok(existing);
        }

        std::fs::create_dir_all(&self.dir).map_err(|source| ClusterError::Io {
            path: self.dir.clone(),
            source,
        })?;
        let node = NodeMetadata {
            format_version: METADATA_FORMAT_VERSION,
            node_id: pinned_node.unwrap_or_else(NodeId::generate),
            created_by_version: version.to_string(),
        };
        let cluster = ClusterMetadata {
            format_version: METADATA_FORMAT_VERSION,
            cluster_id: pinned_cluster.unwrap_or_else(ClusterId::generate),
            created_by_version: version.to_string(),
        };
        write_json(&self.node_path(), &node)?;
        write_json(&self.cluster_path(), &cluster)?;
        Ok(ClusterIdentity { node, cluster })
    }
}

fn check_format(path: &Path, found: u32) -> Result<()> {
    if found > METADATA_FORMAT_VERSION {
        return Err(ClusterError::UnsupportedFormat {
            path: path.to_path_buf(),
            found,
            supported: METADATA_FORMAT_VERSION,
        });
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).map_err(|source| ClusterError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|e| ClusterError::Corrupt {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(|e| ClusterError::Corrupt {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    // Write to a temp file then rename for atomic replacement.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text.as_bytes()).map_err(|source| ClusterError::Io {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| ClusterError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn init_then_load_roundtrips() {
        let dir = tempdir().unwrap();
        let store = ClusterStore::new(dir.path());
        assert!(!store.is_initialized());
        let id = store.init(None, None, "0.4.0").unwrap();
        assert!(store.is_initialized());
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded, id);
    }

    #[test]
    fn ids_persist_across_reopen() {
        let dir = tempdir().unwrap();
        let first = ClusterStore::new(dir.path())
            .init(None, None, "0.4.0")
            .unwrap();
        let second = ClusterStore::new(dir.path()).load().unwrap().unwrap();
        assert_eq!(first.node_id(), second.node_id());
        assert_eq!(first.cluster_id(), second.cluster_id());
    }

    #[test]
    fn init_is_idempotent() {
        let dir = tempdir().unwrap();
        let store = ClusterStore::new(dir.path());
        let a = store.init(None, None, "0.4.0").unwrap();
        let b = store.init(None, None, "0.4.0").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn pinned_id_mismatch_is_rejected() {
        let dir = tempdir().unwrap();
        let store = ClusterStore::new(dir.path());
        store.init(None, None, "0.4.0").unwrap();
        let other = NodeId::from_raw(0x1234);
        assert!(matches!(
            store.init(Some(other), None, "0.4.0"),
            Err(ClusterError::IdentityConflict(_))
        ));
    }

    #[test]
    fn future_format_is_rejected() {
        let dir = tempdir().unwrap();
        let store = ClusterStore::new(dir.path());
        store.init(None, None, "0.4.0").unwrap();
        // Rewrite node.json with a future format version.
        let bad = serde_json::json!({
            "format_version": METADATA_FORMAT_VERSION + 1,
            "node_id": "00000000000000ab",
            "created_by_version": "9.9.9"
        });
        std::fs::write(
            store.node_path(),
            serde_json::to_string_pretty(&bad).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.load(),
            Err(ClusterError::UnsupportedFormat { .. })
        ));
    }

    #[test]
    fn malformed_metadata_is_rejected() {
        let dir = tempdir().unwrap();
        let store = ClusterStore::new(dir.path());
        store.init(None, None, "0.4.0").unwrap();
        std::fs::write(store.node_path(), b"{ not valid json").unwrap();
        assert!(matches!(store.load(), Err(ClusterError::Corrupt { .. })));
    }

    #[test]
    fn partial_identity_is_rejected() {
        let dir = tempdir().unwrap();
        let store = ClusterStore::new(dir.path());
        store.init(None, None, "0.4.0").unwrap();
        std::fs::remove_file(store.cluster_path()).unwrap();
        assert!(store.load().is_err());
    }
}
