//! Integration tests for the live cluster CLI commands.
//!
//! These start a real single-node cluster server (which elects itself leader
//! immediately) and exercise `auradb cluster leader` / `wait-leader` /
//! `wait-ready` against it, plus the timeout path against an unreachable address.

use std::sync::Arc;
use std::time::Duration;

use auradb_cli::{cmd_cluster_leader, cmd_cluster_wait_leader, cmd_cluster_wait_ready};
use auradb_server::{Config, Server};
use tokio::net::TcpListener;
use tokio::sync::Notify;

async fn start_single_node_cluster() -> (String, Arc<Notify>) {
    let dir = tempfile::tempdir().unwrap();
    // A single-node cluster (no peers) elects itself leader immediately and does
    // not start any peer networking.
    let cluster = auradb_cluster::ClusterConfig::single_node();
    let config = Config {
        data_dir: dir.path().to_path_buf(),
        cluster,
        ..Config::default()
    };
    // Keep the tempdir alive for the server's lifetime.
    std::mem::forget(dir);

    let server = Arc::new(Server::open(config).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let shutdown = Arc::new(Notify::new());
    let s2 = Arc::clone(&server);
    let sd = Arc::clone(&shutdown);
    tokio::spawn(async move {
        let _ = s2
            .run_on(listener, async move { sd.notified().await })
            .await;
    });
    (addr, shutdown)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_leader_reports_current_leader() {
    let (addr, shutdown) = start_single_node_cluster().await;
    // wait-ready first so the listener is accepting.
    cmd_cluster_wait_ready(&addr, 10, None, None, "localhost", false)
        .await
        .unwrap();
    let out = cmd_cluster_leader(&addr, None, None, "localhost", false)
        .await
        .unwrap();
    assert!(out.contains("leader:"), "{out}");
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_wait_leader_succeeds() {
    let (addr, shutdown) = start_single_node_cluster().await;
    let out = cmd_cluster_wait_leader(&addr, 10, None, None, "localhost", true)
        .await
        .unwrap();
    assert!(out.contains("leader_id"), "{out}");
    shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_wait_leader_times_out() {
    // Nothing is listening here, so wait-leader must time out (and not hang).
    let unused = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().to_string()
    };
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        cmd_cluster_wait_leader(&unused, 1, None, None, "localhost", false),
    )
    .await
    .expect("the command itself must return within its own timeout");
    assert!(result.is_err(), "wait-leader should time out and error");
}
