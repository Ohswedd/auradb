//! Live cluster diagnostics rendering and warning analysis (v0.6.1).
//!
//! These tests exercise the snapshot-needed and follower-lag diagnostics that
//! `auradb cluster status --addr` and `auradb cluster doctor --addr` surface,
//! using synthetic [`ClusterHealth`] reports so they are deterministic and need
//! no live server.

use auradb_cli::{cluster_health_warnings, format_cluster_status_text};
use auradb_protocol::{ClusterHealth, ClusterPeerHealth, ClusterSnapshotHealth};

fn peer(node_id: &str, catch_up_state: &str) -> ClusterPeerHealth {
    ClusterPeerHealth {
        node_id: node_id.to_string(),
        addr: "127.0.0.1:7100".to_string(),
        client_addr: None,
        connected: true,
        connect_attempts: 0,
        match_index: Some(0),
        next_index: Some(1),
        lag_entries: Some(0),
        needs_snapshot: false,
        snapshot_in_progress: false,
        catch_up_state: catch_up_state.to_string(),
    }
}

fn health(peers: Vec<ClusterPeerHealth>) -> ClusterHealth {
    ClusterHealth {
        enabled: true,
        node_id: Some("00000000000000a1".to_string()),
        cluster_id: Some("00000000c0ffee".to_string()),
        role: "leader".to_string(),
        term: 4,
        leader_id: Some("00000000000000a1".to_string()),
        leader_client_addr: None,
        commit_index: 200,
        applied_index: 200,
        last_log_index: 200,
        peer_count: peers.len(),
        single_node: false,
        replication_lag_entries: 0,
        preview_multi_node: true,
        quorum_available: true,
        peers,
        snapshot: None,
        leader_changes: 0,
    }
}

#[test]
fn status_reports_snapshot_needed() {
    let mut p = peer("00000000000000a2", "snapshot_needed");
    p.needs_snapshot = true;
    p.next_index = Some(5);
    p.lag_entries = Some(150);
    let text = format_cluster_status_text("127.0.0.1:7171", &health(vec![p]));
    assert!(text.contains("catch_up=snapshot_needed"), "{text}");
    assert!(text.contains("needs_snapshot=true"), "{text}");
}

#[test]
fn status_reports_snapshot_in_progress() {
    let mut h = health(vec![peer("00000000000000a2", "snapshot_installing")]);
    h.snapshot = Some(ClusterSnapshotHealth {
        last_included_index: 200,
        last_included_term: 4,
        last_install_unix: Some(1_700_000_000),
        last_error: None,
        bytes_sent: 4096,
        bytes_installed: 0,
        in_progress: 1,
        needed_total: 1,
    });
    let text = format_cluster_status_text("127.0.0.1:7171", &h);
    assert!(text.contains("snapshot: in_progress=1"), "{text}");
    assert!(text.contains("catch_up=snapshot_installing"), "{text}");
}

#[test]
fn status_reports_last_snapshot_error() {
    let mut h = health(vec![peer("00000000000000a2", "normal")]);
    h.snapshot = Some(ClusterSnapshotHealth {
        last_included_index: 0,
        last_included_term: 0,
        last_install_unix: None,
        last_error: Some("snapshot payload digest mismatch".to_string()),
        bytes_sent: 0,
        bytes_installed: 0,
        in_progress: 0,
        needed_total: 0,
    });
    let text = format_cluster_status_text("127.0.0.1:7171", &h);
    assert!(
        text.contains("last_snapshot_error: snapshot payload digest mismatch"),
        "{text}"
    );
}

#[test]
fn status_reports_follower_lag_entries() {
    let mut p = peer("00000000000000a2", "normal");
    p.match_index = Some(150);
    p.next_index = Some(151);
    p.lag_entries = Some(50);
    let text = format_cluster_status_text("127.0.0.1:7171", &health(vec![p]));
    assert!(text.contains("lag_entries=50"), "{text}");
}

#[test]
fn status_reports_follower_catchup_state() {
    let p = peer("00000000000000a2", "caught_up");
    let text = format_cluster_status_text("127.0.0.1:7171", &health(vec![p]));
    assert!(text.contains("catch_up=caught_up"), "{text}");
}

#[test]
fn doctor_warns_snapshot_needed() {
    let mut p = peer("00000000000000a2", "snapshot_needed");
    p.needs_snapshot = true;
    let warnings = cluster_health_warnings(&health(vec![p]));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("need a snapshot install")),
        "{warnings:?}"
    );
}

#[test]
fn doctor_warns_follower_lagging() {
    let mut p = peer("00000000000000a2", "normal");
    p.match_index = Some(120);
    p.lag_entries = Some(80);
    let warnings = cluster_health_warnings(&health(vec![p]));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("is lagging by 80 entries")),
        "{warnings:?}"
    );
}

#[test]
fn doctor_reports_quorum_impact() {
    // A three-node cluster (two peers) with one peer disconnected sits at the
    // minimum quorum: one more loss stalls writes.
    let mut down = peer("00000000000000a2", "unknown");
    down.connected = false;
    down.match_index = None;
    down.next_index = None;
    down.lag_entries = None;
    let up = peer("00000000000000a3", "caught_up");
    let warnings = cluster_health_warnings(&health(vec![down, up]));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("quorum is at the minimum")),
        "{warnings:?}"
    );
}

#[test]
fn doctor_warns_quorum_lost() {
    let mut h = health(vec![
        peer("00000000000000a2", "unknown"),
        peer("00000000000000a3", "unknown"),
    ]);
    h.quorum_available = false;
    let warnings = cluster_health_warnings(&h);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("no quorum is currently reachable")),
        "{warnings:?}"
    );
}

// ---- recovery diagnostics (v0.6.2) ----

#[test]
fn status_reports_leader_changes() {
    let mut h = health(vec![peer("00000000000000a2", "caught_up")]);
    h.leader_changes = 3;
    let text = format_cluster_status_text("127.0.0.1:7171", &h);
    assert!(text.contains("leader_changes: 3"), "{text}");
}

#[test]
fn doctor_warns_reconnect_storm() {
    // A peer still disconnected after many connection attempts is in a reconnect
    // storm.
    let mut p = peer("00000000000000a2", "unknown");
    p.connected = false;
    p.connect_attempts = 42;
    p.match_index = None;
    p.next_index = None;
    p.lag_entries = None;
    let warnings = cluster_health_warnings(&health(vec![p]));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("reconnect storm") && w.contains("42")),
        "{warnings:?}"
    );
}

#[test]
fn doctor_warns_repeated_leader_changes() {
    let mut h = health(vec![peer("00000000000000a2", "caught_up")]);
    h.leader_changes = 17;
    let warnings = cluster_health_warnings(&h);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("leadership has changed 17 times")),
        "{warnings:?}"
    );
}
