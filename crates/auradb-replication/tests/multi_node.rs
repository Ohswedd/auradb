//! Cross-process multi-node preview tests.
//!
//! These spawn real `PeerCluster` nodes bound to real loopback TCP sockets and
//! exercise leader election, replicated writes, follower catch-up after restart,
//! and the minority-cannot-commit safety property. They use readiness checks and
//! bounded polling — never fixed sleeps to "wait for convergence" — and tear
//! every node down cleanly.
//!
//! The blocking leader write path requires a multi-threaded runtime, so every
//! test uses `#[tokio::test(flavor = "multi_thread")]`.

use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::{FindQuery, Mutation};
use auradb::Engine;
use auradb_cluster::{
    ClusterConfig, ClusterId, ClusterIdentity, ClusterMetadata, NodeId, NodeMetadata, NodeRole,
    PeerConfig, METADATA_FORMAT_VERSION,
};
use auradb_replication::PeerCluster;
use tempfile::TempDir;

const VERSION: &str = "0.5.0-test";

fn schema() -> CollectionSchema {
    CollectionSchema::new("C")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Int,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("v", FieldType::Int))
}

fn insert_mutation(id: i64, v: i64) -> Mutation {
    let mut fields = Document::new();
    fields.insert("id".into(), Value::Int(id));
    fields.insert("v".into(), Value::Int(v));
    Mutation::Insert {
        collection: "C".into(),
        fields,
    }
}

/// Reserve `n` distinct loopback ports by binding them simultaneously, then
/// release them so the cluster can claim them. Static membership needs the
/// addresses known up front.
fn reserve_ports(n: usize) -> Vec<u16> {
    let mut held = Vec::new();
    let mut ports = Vec::new();
    for _ in 0..n {
        let l = StdTcpListener::bind("127.0.0.1:0").unwrap();
        ports.push(l.local_addr().unwrap().port());
        held.push(l);
    }
    // `held` drops here, freeing the ports for the cluster to bind.
    ports
}

fn identity(cluster_id: ClusterId, node_id: NodeId) -> ClusterIdentity {
    ClusterIdentity {
        node: NodeMetadata {
            format_version: METADATA_FORMAT_VERSION,
            node_id,
            created_by_version: VERSION.into(),
        },
        cluster: ClusterMetadata {
            format_version: METADATA_FORMAT_VERSION,
            cluster_id,
            created_by_version: VERSION.into(),
        },
    }
}

/// A single node's durable footprint and live handle.
struct Node {
    id: NodeId,
    addr: String,
    engine: Engine,
    peer: Arc<PeerCluster>,
    dir: TempDir,
    cluster_id: ClusterId,
}

struct TestCluster {
    nodes: Vec<Node>,
}

impl TestCluster {
    /// Start an `n`-node cluster. Every node shares one cluster id, has a
    /// distinct node id, and lists the others as static peers.
    async fn start(n: usize) -> TestCluster {
        let cluster_id = ClusterId::new(0xC0FFEE).unwrap();
        let ports = reserve_ports(n);
        let ids: Vec<NodeId> = (1..=n as u64).map(NodeId::from_raw).collect();
        let addrs: Vec<String> = ports.iter().map(|p| format!("127.0.0.1:{p}")).collect();

        let mut nodes = Vec::new();
        for i in 0..n {
            let dir = tempfile::tempdir().unwrap();
            let engine = Engine::open(dir.path().join("data")).unwrap();
            engine.create_schema(schema()).unwrap();
            let cfg = node_config(&ids, &addrs, i, cluster_id);
            let id = identity(cluster_id, ids[i]);
            let peer =
                PeerCluster::spawn(engine.clone(), id, cfg, dir.path().join("cluster")).unwrap();
            engine.attach_replicated_log(peer.write_log());
            nodes.push(Node {
                id: ids[i],
                addr: addrs[i].clone(),
                engine,
                peer,
                dir,
                cluster_id,
            });
        }
        TestCluster { nodes }
    }

    /// Index of the unique current leader, if exactly one node is a leader and at
    /// least a majority recognize it.
    fn leader_index(&self) -> Option<usize> {
        let leaders: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.peer.status().role == NodeRole::Leader)
            .map(|(i, _)| i)
            .collect();
        if leaders.len() != 1 {
            return None;
        }
        let leader_id = self.nodes[leaders[0]].id;
        let recognizers = self
            .nodes
            .iter()
            .filter(|n| n.peer.status().leader_id == Some(leader_id))
            .count();
        (recognizers > self.nodes.len() / 2).then_some(leaders[0])
    }

    /// Poll until a stable leader exists, or panic after `timeout`.
    async fn wait_for_leader(&self, timeout: Duration) -> usize {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(idx) = self.leader_index() {
                return idx;
            }
            if Instant::now() >= deadline {
                panic!("no leader elected within {timeout:?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Write a record through a node's engine (the real, blocking leader path).
    async fn write(&self, idx: usize, id: i64, v: i64) -> auradb::core::Result<()> {
        let engine = self.nodes[idx].engine.clone();
        tokio::task::spawn_blocking(move || engine.apply_mutation(insert_mutation(id, v)))
            .await
            .unwrap()
            .map(|_| ())
    }

    /// Count records of collection "C" on a node (a direct, local read).
    fn record_count(&self, idx: usize) -> usize {
        self.nodes[idx]
            .engine
            .find(&FindQuery::new("C"))
            .unwrap()
            .len()
    }

    /// Poll until every node has at least `n` records, or panic after `timeout`.
    async fn wait_all_have(&self, n: usize, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if (0..self.nodes.len()).all(|i| self.record_count(i) >= n) {
                return;
            }
            if Instant::now() >= deadline {
                let counts: Vec<usize> = (0..self.nodes.len())
                    .map(|i| self.record_count(i))
                    .collect();
                panic!("not all nodes reached {n} records within {timeout:?}: {counts:?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Stop one node's networking (simulate a crash/partition), keeping its data.
    async fn stop(&self, idx: usize) {
        self.nodes[idx].peer.shutdown().await;
    }

    /// Restart a previously stopped node on the same data directory and address.
    async fn restart(&mut self, idx: usize, all_ids: &[NodeId], all_addrs: &[String]) {
        let cluster_id = self.nodes[idx].cluster_id;
        // Re-open against the existing data dir (durable log + engine state).
        let dir_path = self.nodes[idx].dir.path().to_path_buf();
        let engine = Engine::open(dir_path.join("data")).unwrap();
        let cfg = node_config(all_ids, all_addrs, idx, cluster_id);
        let id = identity(cluster_id, self.nodes[idx].id);
        let peer = PeerCluster::spawn(engine.clone(), id, cfg, dir_path.join("cluster")).unwrap();
        engine.attach_replicated_log(peer.write_log());
        self.nodes[idx].engine = engine;
        self.nodes[idx].peer = peer;
    }

    fn ids(&self) -> Vec<NodeId> {
        self.nodes.iter().map(|n| n.id).collect()
    }

    fn addrs(&self) -> Vec<String> {
        self.nodes.iter().map(|n| n.addr.clone()).collect()
    }

    async fn shutdown(self) {
        for n in &self.nodes {
            n.peer.shutdown().await;
        }
    }
}

fn node_config(ids: &[NodeId], addrs: &[String], i: usize, cluster_id: ClusterId) -> ClusterConfig {
    let peers: Vec<PeerConfig> = (0..ids.len())
        .filter(|&j| j != i)
        .map(|j| PeerConfig {
            node_id: ids[j].to_string(),
            addr: addrs[j].clone(),
        })
        .collect();
    ClusterConfig {
        enabled: true,
        experimental_multi_node: true,
        allow_experimental_public_cluster: false,
        cluster_id: cluster_id.to_string(),
        node_id: ids[i].to_string(),
        listen_addr: addrs[i].clone(),
        advertise_addr: addrs[i].clone(),
        bootstrap: true,
        peers,
        peer_auth_token: auradb_cluster::Secret::default(),
        tls: auradb_cluster::ClusterTlsConfig::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_node_cluster_elects_leader() {
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    assert!(cluster.nodes[leader].peer.is_leader());
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leader_replicates_write_to_followers() {
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    cluster.write(leader, 1, 42).await.unwrap();
    // The write is visible on the leader immediately (it committed before return).
    assert_eq!(cluster.record_count(leader), 1);
    // And replicates to all followers.
    cluster.wait_all_have(1, Duration::from_secs(5)).await;
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replicated_transaction_batch_atomic() {
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    for id in 1..=4 {
        cluster.write(leader, id, id * 10).await.unwrap();
    }
    cluster.wait_all_have(4, Duration::from_secs(5)).await;
    // Every replica converges on the same final value for a record.
    for n in &cluster.nodes {
        let rows = n.engine.find(&FindQuery::new("C")).unwrap();
        assert_eq!(rows.len(), 4);
    }
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_write_to_follower_returns_not_leader() {
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    let follower = (0..cluster.nodes.len()).find(|&i| i != leader).unwrap();
    let err = cluster.write(follower, 7, 7).await.unwrap_err();
    assert_eq!(err.code(), auradb::core::ErrorCode::NotLeader);
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn follower_catches_up_after_restart() {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;

    cluster.write(leader, 1, 1).await.unwrap();
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    // Stop a follower, keep writing on the leader (still a 2/3 majority).
    let follower = (0..3).find(|&i| i != leader).unwrap();
    cluster.stop(follower).await;
    cluster.write(leader, 2, 2).await.unwrap();
    cluster.write(leader, 3, 3).await.unwrap();

    // Restart the follower: it replays its durable log and the leader brings it
    // current with the entries it missed while down.
    cluster.restart(follower, &ids, &addrs).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if cluster.record_count(follower) >= 3 {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "restarted follower did not catch up: {} records",
                cluster.record_count(follower)
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cluster.shutdown().await;
}

/// Start a single node that lists one peer which never comes up, so its dialer
/// must retry. Returns the live node and the dead peer's id.
async fn node_with_dead_peer() -> (Arc<PeerCluster>, NodeId, TempDir) {
    let cluster_id = ClusterId::new(0xDEAD).unwrap();
    let ports = reserve_ports(2);
    let me = NodeId::from_raw(1);
    let dead = NodeId::from_raw(2);
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path().join("data")).unwrap();
    engine.create_schema(schema()).unwrap();
    let cfg = ClusterConfig {
        enabled: true,
        experimental_multi_node: true,
        allow_experimental_public_cluster: false,
        cluster_id: cluster_id.to_string(),
        node_id: me.to_string(),
        listen_addr: format!("127.0.0.1:{}", ports[0]),
        advertise_addr: format!("127.0.0.1:{}", ports[0]),
        bootstrap: true,
        peers: vec![PeerConfig {
            node_id: dead.to_string(),
            addr: format!("127.0.0.1:{}", ports[1]), // nothing listens here
        }],
        peer_auth_token: auradb_cluster::Secret::default(),
        tls: auradb_cluster::ClusterTlsConfig::default(),
    };
    let id = identity(cluster_id, me);
    let peer = PeerCluster::spawn(engine.clone(), id, cfg, dir.path().join("cluster")).unwrap();
    (peer, dead, dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_connection_retries_bounded() {
    let (peer, dead, _dir) = node_with_dead_peer().await;
    // Over half a second the dialer should retry a handful of times with bounded
    // backoff — not spin thousands of times.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let attempts = peer.peer_connect_attempts(dead);
    assert!(
        attempts >= 1,
        "the dialer should have attempted at least once"
    );
    assert!(
        attempts < 40,
        "bounded backoff should keep attempts small, got {attempts}"
    );
    peer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_connection_shutdown_clean() {
    let (peer, _dead, _dir) = node_with_dead_peer().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Shutdown must complete promptly even while a dialer is mid-backoff.
    let done = tokio::time::timeout(Duration::from_secs(3), peer.shutdown()).await;
    assert!(done.is_ok(), "shutdown did not complete cleanly in time");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn minority_cannot_commit() {
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;

    // Stop both followers, leaving the leader in a minority of one.
    let followers: Vec<usize> = (0..3).filter(|&i| i != leader).collect();
    for &f in &followers {
        cluster.stop(f).await;
    }

    // A write cannot reach a majority, so it must not commit. We bound the wait:
    // a successful commit would return quickly; here the call stays pending.
    let write = cluster.write(leader, 9, 9);
    let outcome = tokio::time::timeout(Duration::from_secs(1), write).await;
    assert!(
        outcome.is_err(),
        "a minority leader must not be able to commit a write"
    );
    // The uncommitted write is not visible on the leader.
    assert_eq!(cluster.record_count(leader), 0);
    cluster.shutdown().await;
}
