//! Fail-closed behavior around cluster identity and configuration.
//!
//! These tests assert that corrupt, partial, future-format, or self-inconsistent
//! cluster metadata is rejected rather than silently accepted, and that an
//! invalid `[cluster]` configuration fails validation with a clear error. A
//! disabled cluster section never affects single-node behavior.

use auradb_cluster::{ClusterConfig, ClusterError, ClusterStore, NodeId};
use tempfile::tempdir;

fn init(dir: &std::path::Path) {
    ClusterStore::new(dir).init(None, None, "0.4.1").unwrap();
}

#[test]
fn cluster_missing_node_metadata_rejected() {
    let dir = tempdir().unwrap();
    init(dir.path());
    // Remove node.json: a half-present identity must not silently re-initialize.
    std::fs::remove_file(dir.path().join("cluster").join("node.json")).unwrap();
    let err = ClusterStore::new(dir.path()).load().unwrap_err();
    assert!(matches!(err, ClusterError::IdentityConflict(_)), "{err}");
}

#[test]
fn cluster_missing_cluster_metadata_rejected() {
    let dir = tempdir().unwrap();
    init(dir.path());
    std::fs::remove_file(dir.path().join("cluster").join("cluster.json")).unwrap();
    let err = ClusterStore::new(dir.path()).load().unwrap_err();
    assert!(matches!(err, ClusterError::IdentityConflict(_)), "{err}");
}

#[test]
fn cluster_corrupt_node_metadata_rejected() {
    let dir = tempdir().unwrap();
    init(dir.path());
    std::fs::write(dir.path().join("cluster").join("node.json"), b"{ not json").unwrap();
    let err = ClusterStore::new(dir.path()).load().unwrap_err();
    assert!(matches!(err, ClusterError::Corrupt { .. }), "{err}");
}

#[test]
fn cluster_corrupt_cluster_metadata_rejected() {
    let dir = tempdir().unwrap();
    init(dir.path());
    std::fs::write(
        dir.path().join("cluster").join("cluster.json"),
        b"}}garbage{{",
    )
    .unwrap();
    let err = ClusterStore::new(dir.path()).load().unwrap_err();
    assert!(matches!(err, ClusterError::Corrupt { .. }), "{err}");
}

#[test]
fn cluster_future_format_rejected() {
    let dir = tempdir().unwrap();
    init(dir.path());
    let path = dir.path().join("cluster").join("node.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let bumped = text.replace("\"format_version\": 1", "\"format_version\": 9999");
    assert_ne!(text, bumped, "patched the format version");
    std::fs::write(&path, bumped).unwrap();
    let err = ClusterStore::new(dir.path()).load().unwrap_err();
    assert!(
        matches!(err, ClusterError::UnsupportedFormat { .. }),
        "{err}"
    );
}

#[test]
fn cluster_node_id_mismatch_rejected() {
    let dir = tempdir().unwrap();
    let store = ClusterStore::new(dir.path());
    store.init(None, None, "0.4.1").unwrap();
    // Re-initializing with a different pinned node id is a hard conflict.
    let other = NodeId::from_raw(0xdead_beef);
    let err = store.init(Some(other), None, "0.4.1").unwrap_err();
    assert!(matches!(err, ClusterError::IdentityConflict(_)), "{err}");
}

fn peer(node_id: &str, addr: &str) -> auradb_cluster::PeerConfig {
    auradb_cluster::PeerConfig {
        node_id: node_id.to_string(),
        addr: addr.to_string(),
        client_addr: None,
    }
}

#[test]
fn cluster_duplicate_peer_rejected() {
    let mut cfg = ClusterConfig::single_node();
    cfg.experimental_multi_node = true;
    cfg.peers = vec![
        peer("00000000000000a2", "127.0.0.1:7272"),
        peer("00000000000000a3", "127.0.0.1:7272"),
    ];
    assert!(cfg.validate().is_err());
}

#[test]
fn cluster_self_peer_rejected() {
    let mut cfg = ClusterConfig::single_node();
    cfg.experimental_multi_node = true;
    cfg.listen_addr = "127.0.0.1:7172".into();
    cfg.advertise_addr = "127.0.0.1:7172".into();
    cfg.peers = vec![peer("00000000000000a2", "127.0.0.1:7172")];
    assert!(cfg.validate().is_err());
}

#[test]
fn cluster_invalid_peer_address_rejected() {
    let mut cfg = ClusterConfig::single_node();
    cfg.experimental_multi_node = true;
    cfg.peers = vec![peer("00000000000000a2", "nope")];
    assert!(cfg.validate().is_err());
}

#[test]
fn cluster_disabled_ignores_missing_cluster_metadata() {
    // A disabled cluster section validates trivially and does not require any
    // on-disk identity: single-node behavior is unaffected.
    let cfg = ClusterConfig::default();
    assert!(!cfg.enabled);
    cfg.validate().unwrap();
    let dir = tempdir().unwrap();
    assert!(!ClusterStore::new(dir.path()).is_initialized());
    // Loading an uninitialized store is `Ok(None)`, not an error.
    assert!(ClusterStore::new(dir.path()).load().unwrap().is_none());
}
