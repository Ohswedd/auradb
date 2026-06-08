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

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{CompareOp, Filter, FindQuery, Mutation, VectorSearch};
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
        .with_field(FieldDef {
            name: "v".into(),
            field_type: FieldType::Int,
            primary_key: false,
            unique: false,
            nullable: false,
            indexed: true,
        })
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
        Self::start_with(n, schema()).await
    }

    /// Start an `n`-node cluster whose collection uses `collection_schema` (the
    /// scalar [`schema`] or the multi-model [`rich_schema`]).
    async fn start_with(n: usize, collection_schema: CollectionSchema) -> TestCluster {
        Self::start_cfg(n, collection_schema, true).await
    }

    /// Start an `n`-node cluster, optionally declaring each node's client address
    /// (its own `advertise_client_addr` and the peers' `client_addr`). Passing
    /// `false` exercises the honest "leader client address unknown" path.
    async fn start_cfg(
        n: usize,
        collection_schema: CollectionSchema,
        declare_client_addrs: bool,
    ) -> TestCluster {
        let cluster_id = ClusterId::new(0xC0FFEE).unwrap();
        let ports = reserve_ports(n);
        let ids: Vec<NodeId> = (1..=n as u64).map(NodeId::from_raw).collect();
        let addrs: Vec<String> = ports.iter().map(|p| format!("127.0.0.1:{p}")).collect();

        let mut nodes = Vec::new();
        for i in 0..n {
            let dir = tempfile::tempdir().unwrap();
            let engine = Engine::open(dir.path().join("data")).unwrap();
            engine.create_schema(collection_schema.clone()).unwrap();
            let cfg = node_config_with(&ids, &addrs, i, cluster_id, declare_client_addrs);
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

    /// A unique leader among the given `live` node indices, recognized by a
    /// majority of those live nodes. Used after one or more nodes are stopped:
    /// a stopped node retains a stale `role`/`leader_id`, so it must be excluded
    /// from both the leader check and the recognizer count.
    fn live_leader_index(&self, live: &[usize]) -> Option<usize> {
        let leaders: Vec<usize> = live
            .iter()
            .copied()
            .filter(|&i| self.nodes[i].peer.status().role == NodeRole::Leader)
            .collect();
        if leaders.len() != 1 {
            return None;
        }
        let leader_id = self.nodes[leaders[0]].id;
        let recognizers = live
            .iter()
            .copied()
            .filter(|&i| self.nodes[i].peer.status().leader_id == Some(leader_id))
            .count();
        (recognizers > live.len() / 2).then_some(leaders[0])
    }

    /// Poll until a stable leader exists among `live`, or panic after `timeout`.
    async fn wait_for_live_leader(&self, live: &[usize], timeout: Duration) -> usize {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(idx) = self.live_leader_index(live) {
                return idx;
            }
            if Instant::now() >= deadline {
                panic!("no leader elected among {live:?} within {timeout:?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Poll until every node in `idxs` has at least `n` records, or panic.
    async fn wait_indices_have(&self, idxs: &[usize], n: usize, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if idxs.iter().all(|&i| self.record_count(i) >= n) {
                return;
            }
            if Instant::now() >= deadline {
                let counts: Vec<usize> = idxs.iter().map(|&i| self.record_count(i)).collect();
                panic!("nodes {idxs:?} did not reach {n} records within {timeout:?}: {counts:?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Write a record through a node's engine (the real, blocking leader path).
    ///
    /// A single synchronous commit waits for a majority within a fixed timeout.
    /// Under heavy CI parallelism the Raft driver task can be starved of CPU long
    /// enough that one commit exceeds that timeout, or leadership briefly churns
    /// during a restart test — both transient resource conditions, not
    /// correctness failures. Retry those a bounded number of times so the tests
    /// are not flaky on contended runners; a genuinely stuck cluster still fails
    /// after the retries are exhausted.
    async fn write(&self, idx: usize, id: i64, v: i64) -> auradb::core::Result<()> {
        for attempt in 0..6 {
            let engine = self.nodes[idx].engine.clone();
            let result =
                tokio::task::spawn_blocking(move || engine.apply_mutation(insert_mutation(id, v)))
                    .await
                    .unwrap();
            match result {
                Ok(_) => return Ok(()),
                Err(err) => {
                    let transient = matches!(&err, auradb::core::Error::NotLeader(_))
                        || matches!(&err, auradb::core::Error::Internal(m)
                            if m.contains("replication timed out"));
                    if transient && attempt < 5 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        unreachable!("write retry loop always returns")
    }

    /// Commit a write through whichever of `live` is currently the leader,
    /// re-resolving the leader and retrying on transient conditions (`NotLeader`
    /// or a commit timeout). Unlike [`write`], this does not assume a fixed node
    /// stays leader: on a slow/contended runner a heartbeat hiccup can move
    /// leadership mid-test, so a "write to the cluster" must follow the leader.
    /// Panics only if no write can be committed within a generous deadline.
    async fn write_via_leader(&self, live: &[usize], id: i64, v: i64) {
        let deadline = Instant::now() + Duration::from_secs(30);
        // Once a proposal may have committed but its ack was lost (a transient
        // timeout / leadership churn), a retry can re-observe *its own* prior
        // write as a primary-key conflict. That means the write did land, so
        // treat a conflict after a transient attempt as success (at-least-once).
        let mut may_have_committed = false;
        loop {
            if let Some(li) = self.live_leader_index(live) {
                let engine = self.nodes[li].engine.clone();
                let result = tokio::task::spawn_blocking(move || {
                    engine.apply_mutation(insert_mutation(id, v))
                })
                .await
                .unwrap();
                match result {
                    Ok(_) => return,
                    Err(auradb::core::Error::UniqueViolation(_)) if may_have_committed => return,
                    Err(err) => {
                        let transient = matches!(&err, auradb::core::Error::NotLeader(_))
                            || matches!(&err, auradb::core::Error::Internal(m)
                                if m.contains("replication timed out"));
                        if !transient {
                            panic!("write {id} via leader {li} failed: {err}");
                        }
                        may_have_committed = true;
                    }
                }
            }
            if Instant::now() >= deadline {
                panic!("could not commit write {id} via a leader among {live:?} within 30s");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
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

/// Synthetic, deterministic client address for node `i`. The multi-node harness
/// drives the engine directly and never binds a client listener, so this string
/// only has to be a valid, distinct `host:port` for the leader-hint lookup to
/// resolve — it is deliberately disjoint from the (random) cluster transport
/// ports so a test can assert the hint is the client address, never the peer
/// transport address.
fn client_addr_of(i: usize) -> String {
    format!("127.0.0.1:{}", 7900 + i)
}

fn node_config(ids: &[NodeId], addrs: &[String], i: usize, cluster_id: ClusterId) -> ClusterConfig {
    node_config_with(ids, addrs, i, cluster_id, true)
}

/// Like [`node_config`] but lets a test omit declared client addresses, so the
/// honest "leader client address unknown" fallback path can be exercised.
fn node_config_with(
    ids: &[NodeId],
    addrs: &[String],
    i: usize,
    cluster_id: ClusterId,
    declare_client_addrs: bool,
) -> ClusterConfig {
    let peers: Vec<PeerConfig> = (0..ids.len())
        .filter(|&j| j != i)
        .map(|j| PeerConfig {
            node_id: ids[j].to_string(),
            addr: addrs[j].clone(),
            client_addr: declare_client_addrs.then(|| client_addr_of(j)),
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
        advertise_client_addr: declare_client_addrs.then(|| client_addr_of(i)),
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

// ----- leader-hint (client_addr) propagation (v0.9.1) -----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_leader_hint_does_not_use_peer_addr_as_client_addr() {
    // The leader hint is the operator-declared client address, never a node's
    // cluster *transport* address.
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(15)).await;
    let follower = (0..3).find(|&i| i != leader).unwrap();

    let hint = cluster.nodes[follower]
        .peer
        .leader_client_addr()
        .expect("follower reports the leader's declared client address");
    assert_eq!(
        hint,
        client_addr_of(leader),
        "hint is the leader's client_addr"
    );

    // It must not be any node's transport address.
    let transports = cluster.addrs();
    assert!(
        !transports.contains(&hint),
        "leader hint {hint} must not be a peer transport address: {transports:?}"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_leader_hint_omits_unknown_client_addr_safely() {
    // With no client addresses declared anywhere, the hint is honestly absent on
    // both the leader (its own advertise_client_addr unset) and a follower (the
    // leader peer's client_addr unset) — never guessed, never a transport addr.
    let cluster = TestCluster::start_cfg(3, schema(), false).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(15)).await;
    let follower = (0..3).find(|&i| i != leader).unwrap();

    assert!(
        cluster.nodes[leader].peer.leader_client_addr().is_none(),
        "leader has no declared own client address -> unknown"
    );
    assert!(
        cluster.nodes[follower].peer.leader_client_addr().is_none(),
        "follower's leader peer has no declared client address -> unknown"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn not_leader_includes_leader_client_addr_after_re_election() {
    // After a leader change, the leader hint follows the NEW leader: the new
    // leader names its own client address (the v0.9.1 self-report), and a
    // surviving follower converges on that same address, not the stopped leader's.
    let cluster = TestCluster::start(3).await;
    let old_leader = cluster.wait_for_leader(Duration::from_secs(15)).await;
    cluster
        .write_via_leader(&(0..3).collect::<Vec<_>>(), 1, 1)
        .await;

    cluster.stop(old_leader).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != old_leader).collect();
    let new_leader = cluster
        .wait_for_live_leader(&live, Duration::from_secs(15))
        .await;
    let expected = client_addr_of(new_leader);

    // The new leader reports its OWN client address (a peer never could).
    assert_eq!(
        cluster.nodes[new_leader]
            .peer
            .leader_client_addr()
            .as_deref(),
        Some(expected.as_str()),
        "the new leader reports its own client address as the hint"
    );
    assert_ne!(
        expected,
        client_addr_of(old_leader),
        "the hint moved off the stopped old leader"
    );

    // A surviving follower converges on the same new-leader client address.
    let follower = live.iter().copied().find(|&i| i != new_leader).unwrap();
    let new_leader_id = cluster.nodes[new_leader].id;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if cluster.nodes[follower].peer.status().leader_id == Some(new_leader_id) {
            break;
        }
        if Instant::now() >= deadline {
            panic!("follower did not recognize the new leader in time");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        cluster.nodes[follower].peer.leader_client_addr().as_deref(),
        Some(expected.as_str()),
        "follower's hint matches the new leader's client address"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn follower_catches_up_after_restart() {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;

    cluster.write_via_leader(&[0, 1, 2], 1, 1).await;
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    // Stop a follower, keep writing via the current leader (still a 2/3 majority).
    let follower = (0..3).find(|&i| i != leader).unwrap();
    cluster.stop(follower).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != follower).collect();
    cluster.write_via_leader(&live, 2, 2).await;
    cluster.write_via_leader(&live, 3, 3).await;

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
/// must retry. Returns the live node, the dead peer's id, this node's listen
/// address, the cluster id, and the data dir guard.
async fn node_with_dead_peer() -> (Arc<PeerCluster>, NodeId, String, ClusterId, TempDir, Engine) {
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
        advertise_client_addr: None,
        bootstrap: true,
        peers: vec![PeerConfig {
            node_id: dead.to_string(),
            addr: format!("127.0.0.1:{}", ports[1]), // nothing listens here
            client_addr: None,
        }],
        peer_auth_token: auradb_cluster::Secret::default(),
        tls: auradb_cluster::ClusterTlsConfig::default(),
    };
    let id = identity(cluster_id, me);
    let listen_addr = format!("127.0.0.1:{}", ports[0]);
    let peer = PeerCluster::spawn(engine.clone(), id, cfg, dir.path().join("cluster")).unwrap();
    (peer, dead, listen_addr, cluster_id, dir, engine)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_connection_retries_bounded() {
    let (peer, dead, _addr, _cid, _dir, _engine) = node_with_dead_peer().await;
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
    let (peer, _dead, _addr, _cid, _dir, _engine) = node_with_dead_peer().await;
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

// ---- diagnostics (v0.5.1) ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_status_reports_unreachable_peer() {
    let (peer, dead, _addr, _cid, _dir, _engine) = node_with_dead_peer().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let statuses = peer.peer_status();
    let s = statuses
        .iter()
        .find(|s| s.node_id == dead.to_string())
        .expect("the dead peer is listed in diagnostics");
    assert!(
        !s.connected,
        "an unreachable peer is reported as not connected"
    );
    assert!(
        s.connect_attempts >= 1,
        "diagnostics report outbound connection attempts: {}",
        s.connect_attempts
    );
    // With only one of two voters reachable (this node), no quorum is available.
    assert!(
        !peer.quorum_available(),
        "a lone node with a single dead peer has no quorum"
    );
    peer.shutdown().await;
}

// ---- leader restart and re-election (v0.5.1 preview) ----

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn leader_restart_elects_new_leader() {
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    cluster.write_via_leader(&[0, 1, 2], 1, 1).await;
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    // Stop the leader; the surviving majority must elect a new leader.
    cluster.stop(leader).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != leader).collect();
    let new_leader = cluster
        .wait_for_live_leader(&live, Duration::from_secs(10))
        .await;
    assert_ne!(new_leader, leader, "a different node takes over leadership");
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn write_continues_after_leader_restart_with_majority() {
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    cluster.write_via_leader(&[0, 1, 2], 1, 1).await;
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    cluster.stop(leader).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != leader).collect();
    cluster
        .wait_for_live_leader(&live, Duration::from_secs(10))
        .await;

    // The new leader accepts writes (a 2/3 majority is still present).
    cluster.write_via_leader(&live, 2, 2).await;
    cluster
        .wait_indices_have(&live, 2, Duration::from_secs(5))
        .await;
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn old_leader_rejoins_as_follower() {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;

    cluster.write_via_leader(&[0, 1, 2], 1, 1).await;
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    cluster.stop(leader).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != leader).collect();
    cluster
        .wait_for_live_leader(&live, Duration::from_secs(10))
        .await;

    // Commit more writes while the old leader is down so its log falls behind the
    // committed prefix. Raft's leader-completeness then guarantees it cannot win
    // a future election (a node missing committed entries is denied votes), so on
    // restart it deterministically rejoins as a follower rather than possibly
    // winning re-election (which an up-to-date node legitimately could).
    cluster.write_via_leader(&live, 2, 2).await;
    cluster.write_via_leader(&live, 3, 3).await;
    cluster
        .wait_indices_have(&live, 3, Duration::from_secs(10))
        .await;

    // Restart the old leader; it discovers the cluster has moved on and steps
    // down. Eventually the full cluster settles on a single leader and the
    // rejoined node is a follower recognizing some leader other than itself.
    cluster.restart(leader, &ids, &addrs).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let st = cluster.nodes[leader].peer.status();
        let settled = cluster.leader_index().is_some();
        if settled
            && st.role == NodeRole::Follower
            && matches!(st.leader_id, Some(id) if id != cluster.nodes[leader].id)
        {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "old leader did not rejoin as a follower (role {:?}, leader_id {:?})",
                st.role, st.leader_id
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn old_leader_catches_up_after_restart() {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    cluster.write_via_leader(&[0, 1, 2], 1, 1).await;
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    // Stop the leader; the majority elects a new one and accepts more writes.
    cluster.stop(leader).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != leader).collect();
    cluster
        .wait_for_live_leader(&live, Duration::from_secs(10))
        .await;
    cluster.write_via_leader(&live, 2, 2).await;
    cluster.write_via_leader(&live, 3, 3).await;
    cluster
        .wait_indices_have(&live, 3, Duration::from_secs(5))
        .await;

    // Restart the old leader; it catches up on the entries committed while down.
    cluster.restart(leader, &ids, &addrs).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if cluster.record_count(leader) >= 3 {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "restarted old leader did not catch up: {} records",
                cluster.record_count(leader)
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn all_nodes_consistent_after_leader_restart() {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    cluster.write_via_leader(&[0, 1, 2], 1, 1).await;
    // Let the first write settle on every node so the post-stop election is
    // deterministic (any survivor has an up-to-date log and can win).
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    cluster.stop(leader).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != leader).collect();
    cluster
        .wait_for_live_leader(&live, Duration::from_secs(10))
        .await;
    cluster.write_via_leader(&live, 2, 2).await;
    cluster.write_via_leader(&live, 3, 3).await;

    // Restart the old leader and wait until every node has all three records.
    cluster.restart(leader, &ids, &addrs).await;
    cluster.wait_all_have(3, Duration::from_secs(10)).await;

    // Every node holds an identical record set.
    let expected: std::collections::BTreeSet<i64> = [1, 2, 3].into_iter().collect();
    for i in 0..3 {
        let ids_on_node: std::collections::BTreeSet<i64> = cluster.nodes[i]
            .engine
            .find(&FindQuery::new("C"))
            .unwrap()
            .into_iter()
            .map(|r| match r.fields.get("id") {
                Some(Value::Int(v)) => *v,
                other => panic!("unexpected id value {other:?}"),
            })
            .collect();
        assert_eq!(ids_on_node, expected, "node {i} converged on the same data");
    }
    cluster.shutdown().await;
}

// ---- follower catch-up under larger logs (v0.5.1) ----

// The 1,000-entry variant is the heaviest cluster test: it commits a thousand
// synchronous, majority-acknowledged writes through the real leader path. On a
// contended CI runner (few cores) running the whole multi-node suite in
// parallel, the Raft driver tasks can be starved enough that an individual
// write exceeds its commit timeout. It passes reliably when run on its own, so
// it is `#[ignore]`d by default and run on demand with `-- --ignored`. The
// lighter `follower_catches_up_with_transaction_batches` and
// `follower_catches_up_with_snapshot_boundary_present` variants remain part of
// the default (required) suite and cover the same catch-up path.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "heavy: 1000 synchronous commits; run with `cargo test -- --ignored`"]
async fn follower_catches_up_after_1000_entries() {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;

    // Stop a follower; the remaining majority commits a long run via whichever
    // node is currently the leader.
    let follower = (0..3).find(|&i| i != leader).unwrap();
    cluster.stop(follower).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != follower).collect();

    const N: i64 = 1000;
    for id in 1..=N {
        cluster.write_via_leader(&live, id, id).await;
    }

    // Restart the follower; it replays its durable log and the leader brings it
    // current with everything committed while it was down.
    cluster.restart(follower, &ids, &addrs).await;
    cluster
        .wait_indices_have(&[follower], N as usize, Duration::from_secs(30))
        .await;

    // The follower's applied index catches up to the rest of the cluster (no gap).
    let leader_applied = live
        .iter()
        .map(|&i| cluster.nodes[i].peer.status().applied_index)
        .max()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if cluster.nodes[follower].peer.status().applied_index >= leader_applied {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "follower applied index {} did not reach leader's {}",
                cluster.nodes[follower].peer.status().applied_index,
                leader_applied
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn follower_catches_up_with_transaction_batches() {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;

    // Establish a baseline that all three share.
    cluster.write_via_leader(&[0, 1, 2], 1, 1).await;
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    // Stop a follower and commit several more batches via the current leader.
    let follower = (0..3).find(|&i| i != leader).unwrap();
    cluster.stop(follower).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != follower).collect();
    for id in 2..=60 {
        cluster.write_via_leader(&live, id, id * 7).await;
    }

    // The restarted follower converges on the full set, values intact.
    cluster.restart(follower, &ids, &addrs).await;
    cluster
        .wait_indices_have(&[follower], 60, Duration::from_secs(20))
        .await;
    let rows = cluster.nodes[follower]
        .engine
        .find(&FindQuery::new("C"))
        .unwrap();
    assert_eq!(rows.len(), 60, "follower holds every batched record");
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn follower_catches_up_with_snapshot_boundary_present() {
    // The multi-node preview retains the full Raft log under an active static
    // membership (it does not compact the log out from under a live follower), so
    // catch-up is always via log replay across the commit-base/snapshot boundary
    // rather than a snapshot install. This exercises catch-up across that
    // boundary with a non-trivial committed prefix.
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;

    for id in 1..=20 {
        cluster.write_via_leader(&[0, 1, 2], id, id).await;
    }
    cluster.wait_all_have(20, Duration::from_secs(10)).await;

    let follower = (0..3).find(|&i| i != leader).unwrap();
    cluster.stop(follower).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != follower).collect();
    for id in 21..=120 {
        cluster.write_via_leader(&live, id, id).await;
    }
    cluster.restart(follower, &ids, &addrs).await;
    cluster
        .wait_indices_have(&[follower], 120, Duration::from_secs(20))
        .await;
    cluster.shutdown().await;
}

// ----- peer snapshot install (v0.6.0) -----

/// Drive a follower behind the leader's compacted prefix so it can only be
/// brought current by a snapshot install. Returns the running cluster, the
/// follower index, and the committed record count it must reach.
async fn run_snapshot_install_scenario() -> (TestCluster, usize, usize) {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    let all: Vec<usize> = (0..3).collect();

    // Seed a baseline every node holds.
    for id in 1..=20 {
        cluster.write_via_leader(&all, id, id).await;
    }
    cluster.wait_all_have(20, Duration::from_secs(10)).await;

    // Stop one follower and commit a long run it will miss.
    let follower = (0..3).find(|&i| i != leader).unwrap();
    cluster.stop(follower).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != follower).collect();
    let target = 120usize;
    for id in 21..=target as i64 {
        cluster.write_via_leader(&live, id, id).await;
    }
    cluster
        .wait_indices_have(&live, target, Duration::from_secs(20))
        .await;

    // Compact the live nodes' logs so the entries the follower needs are gone:
    // whichever node leads can now only serve the follower via a snapshot
    // install, not via AppendEntries.
    for &i in &live {
        let _ = cluster.nodes[i].peer.compact_log();
    }

    // Bring the follower back; it must be caught up by a snapshot install.
    cluster.restart(follower, &ids, &addrs).await;
    (cluster, follower, target)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn install_snapshot_restores_follower_after_compaction() {
    let (cluster, follower, target) = run_snapshot_install_scenario().await;
    // The follower caught up to the full committed state.
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(30))
        .await;
    // It was brought current by an actual snapshot install (not AppendEntries):
    // since both live nodes compacted the entries the follower needed, the only
    // possible catch-up path is a snapshot install. Poll the counters with a
    // bounded deadline — the follower's engine reaches `target` records inside the
    // install handler a hair before the install counter is incremented, so a
    // single read right after catch-up can race that increment.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let installed = cluster.nodes[follower].peer.snapshot_counters().1;
        let sent: u64 = (0..3)
            .map(|i| cluster.nodes[i].peer.snapshot_counters().0)
            .sum();
        if installed >= 1 && sent >= 1 {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "expected a snapshot install: follower installed={installed}, cluster sent={sent}"
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn append_entries_resume_after_snapshot_install() {
    let (cluster, follower, target) = run_snapshot_install_scenario().await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(30))
        .await;
    // After the snapshot install, normal AppendEntries replication resumes: new
    // writes committed by the leader reach the recovered follower too.
    let all: Vec<usize> = (0..3).collect();
    for id in (target as i64 + 1)..=(target as i64 + 10) {
        cluster.write_via_leader(&all, id, id).await;
    }
    cluster
        .wait_indices_have(&[follower], target + 10, Duration::from_secs(30))
        .await;
    cluster.shutdown().await;
}

// ----- snapshot / compaction across a leader change (v0.9.1) -----

/// Baseline on all three; stop the leader (a real leader change); commit a run
/// the old leader misses on the surviving majority; compact the survivors past
/// those entries; restart the old leader so it can only be brought current by a
/// snapshot install. Returns the cluster, the (rejoined) old-leader index, the
/// new-leader index, and the committed record count to reach.
async fn snapshot_after_leader_change_scenario() -> (TestCluster, usize, usize, usize) {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let old_leader = cluster.wait_for_leader(Duration::from_secs(15)).await;
    let all: Vec<usize> = (0..3).collect();

    for id in 1..=20 {
        cluster.write_via_leader(&all, id, id).await;
    }
    cluster.wait_all_have(20, Duration::from_secs(15)).await;

    // Leader change: stop the leader; the surviving two elect a new one.
    cluster.stop(old_leader).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != old_leader).collect();
    let new_leader = cluster
        .wait_for_live_leader(&live, Duration::from_secs(15))
        .await;

    // Commit a run the old leader misses, then compact the survivors so only a
    // snapshot install (not AppendEntries) can bring it current on rejoin.
    let target = 120usize;
    for id in 21..=target as i64 {
        cluster.write_via_leader(&live, id, id).await;
    }
    cluster
        .wait_indices_have(&live, target, Duration::from_secs(25))
        .await;
    for &i in &live {
        let _ = cluster.nodes[i].peer.compact_log();
    }

    // Rejoin the old leader; it returns as a follower and catches up by snapshot.
    cluster.restart(old_leader, &ids, &addrs).await;
    (cluster, old_leader, new_leader, target)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn snapshot_install_after_leader_change() {
    let (cluster, old_leader, _new_leader, target) = snapshot_after_leader_change_scenario().await;
    // The rejoined old leader catches up to the full committed state.
    cluster
        .wait_indices_have(&[old_leader], target, Duration::from_secs(30))
        .await;
    // Catch-up was a real snapshot install (the survivors compacted the entries
    // it needed). Poll the counters with a bounded deadline to avoid racing the
    // install-counter increment with the engine reaching `target`.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let installed = cluster.nodes[old_leader].peer.snapshot_counters().1;
        let sent: u64 = (0..3)
            .map(|i| cluster.nodes[i].peer.snapshot_counters().0)
            .sum();
        if installed >= 1 && sent >= 1 {
            break;
        }
        if Instant::now() >= deadline {
            panic!("expected a snapshot install after leader change: installed={installed} sent={sent}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn old_leader_rejoins_then_receives_snapshot_if_needed() {
    let (cluster, old_leader, _new_leader, target) = snapshot_after_leader_change_scenario().await;
    // The old leader rejoins as a follower recognizing some current leader other
    // than itself. (Which survivor leads can shift again during the rejoin, so we
    // do not pin it to the first-elected node — only that the old leader stepped
    // down and follows the cluster.)
    let old_leader_id = cluster.nodes[old_leader].id;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let st = cluster.nodes[old_leader].peer.status();
        if st.role == NodeRole::Follower && matches!(st.leader_id, Some(id) if id != old_leader_id)
        {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "old leader did not rejoin as a follower of another node (role {:?}, leader {:?})",
                st.role, st.leader_id
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // And it is brought fully current (here, by the snapshot it needed).
    cluster
        .wait_indices_have(&[old_leader], target, Duration::from_secs(30))
        .await;
    assert_eq!(cluster.record_count(old_leader), target);
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn snapshot_metrics_after_leader_change() {
    let (cluster, old_leader, _new_leader, target) = snapshot_after_leader_change_scenario().await;
    cluster
        .wait_indices_have(&[old_leader], target, Duration::from_secs(30))
        .await;
    // Diagnostics are consistent across the leader change: the rejoined node
    // records installed bytes and an install boundary, some live node shipped
    // bytes, and a leader change was observed — with no apply errors.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let diag = cluster.nodes[old_leader].peer.snapshot_diagnostics();
        let sent_bytes: u64 = (0..3)
            .map(|i| cluster.nodes[i].peer.snapshot_diagnostics().bytes_sent)
            .sum();
        if diag.bytes_installed > 0 && sent_bytes > 0 {
            assert!(
                diag.last_included_index > 0,
                "diagnostics record the installed boundary"
            );
            break;
        }
        if Instant::now() >= deadline {
            panic!("snapshot metrics did not advance after leader change: {diag:?}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        cluster.max_leader_changes() >= 1,
        "a leader change should be recorded"
    );
    assert_eq!(
        cluster.nodes[old_leader].peer.metrics().apply_errors,
        0,
        "snapshot install + resume after a leader change must not produce apply errors"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn compaction_after_leader_change() {
    // After a leader change, the NEW leader can compact its log and keep serving
    // writes — compaction state survives the role change.
    let cluster = TestCluster::start(3).await;
    let old_leader = cluster.wait_for_leader(Duration::from_secs(15)).await;
    let all: Vec<usize> = (0..3).collect();
    for id in 1..=20 {
        cluster.write_via_leader(&all, id, id).await;
    }
    cluster.wait_all_have(20, Duration::from_secs(15)).await;

    cluster.stop(old_leader).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != old_leader).collect();
    let new_leader = cluster
        .wait_for_live_leader(&live, Duration::from_secs(15))
        .await;
    // Commit more so the new leader has a log to compact.
    for id in 21..=60 {
        cluster.write_via_leader(&live, id, id).await;
    }
    cluster
        .wait_indices_have(&live, 60, Duration::from_secs(25))
        .await;

    // The new leader compacts successfully...
    cluster.nodes[new_leader]
        .peer
        .compact_log()
        .expect("new leader compacts its log after the leader change");
    // ...and still commits new writes afterwards.
    cluster.write_via_leader(&live, 61, 61).await;
    cluster
        .wait_indices_have(&live, 61, Duration::from_secs(25))
        .await;
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn snapshot_failure_after_leader_change_safe_to_retry() {
    // A corrupt snapshot install delivered to the NEW leader after a leader change
    // is rejected safely: existing committed state is preserved and nothing is
    // recorded as installed, and a retry is rejected the same way (idempotent).
    use auradb_replication::transport::{self, PeerMessage};

    let cluster = TestCluster::start(3).await;
    let old_leader = cluster.wait_for_leader(Duration::from_secs(15)).await;
    let all: Vec<usize> = (0..3).collect();
    for id in 1..=5 {
        cluster.write_via_leader(&all, id, id).await;
    }
    cluster.wait_all_have(5, Duration::from_secs(15)).await;

    cluster.stop(old_leader).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != old_leader).collect();
    let new_leader = cluster
        .wait_for_live_leader(&live, Duration::from_secs(15))
        .await;
    let cluster_id = cluster.nodes[new_leader].cluster_id;
    let target_addr = cluster.nodes[new_leader].addr.clone();
    // Impersonate the stopped old leader: it is a configured peer of the new
    // leader and is offline, so there is no live duplicate-connection conflict.
    let impersonate = cluster.nodes[old_leader].id;
    let before = cluster.record_count(new_leader);
    assert!(before >= 5);

    // Deliver a corrupt install twice; each time it must be rejected (the rejected
    // counter advances), committed state stays intact, and nothing is installed.
    for attempt in 0..2 {
        let mut manifest = sample_manifest(cluster_id);
        manifest.payload.push(0xFF); // corrupt the payload so decode/verify fails
        let req = PeerMessage::InstallSnapshotRequest {
            from: impersonate,
            term: u64::MAX, // never stale, so the corruption is what rejects it
            last_included_index: 5,
            last_included_term: 1,
            snapshot: manifest.encode().unwrap(),
        };
        let mut stream = handshake_as_peer(&target_addr, cluster_id, impersonate).await;
        let rejected_before = cluster.nodes[new_leader].peer.snapshot_counters().2;
        transport::write_message(&mut stream, &req).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if cluster.nodes[new_leader].peer.snapshot_counters().2 > rejected_before {
                break;
            }
            if Instant::now() >= deadline {
                panic!("attempt {attempt}: new leader did not reject the corrupt install");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(
            cluster.record_count(new_leader),
            before,
            "attempt {attempt}: committed state must be preserved"
        );
        assert_eq!(
            cluster.nodes[new_leader].peer.snapshot_counters().1,
            0,
            "attempt {attempt}: a rejected install must not count as installed"
        );
    }
    cluster.shutdown().await;
}

// ----- larger and concurrent snapshot install scenarios (v0.6.1) -----

/// Like [`run_snapshot_install_scenario`] but parameterized on the shared
/// `baseline` count and the post-stop `target` count, so larger catch-up runs
/// (and the ignored stress run) reuse one well-tested scenario. The follower is
/// stopped after the baseline, the live majority commits up to `target`, the
/// live logs are compacted past the entries the follower needs, and the follower
/// is restarted so it can only be brought current by a snapshot install.
async fn snapshot_install_scenario_sized(
    baseline: i64,
    target: i64,
    catch_up_timeout: Duration,
) -> (TestCluster, usize, usize) {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    let all: Vec<usize> = (0..3).collect();

    for id in 1..=baseline {
        cluster.write_via_leader(&all, id, id).await;
    }
    cluster
        .wait_all_have(baseline as usize, Duration::from_secs(15))
        .await;

    let follower = (0..3).find(|&i| i != leader).unwrap();
    cluster.stop(follower).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != follower).collect();
    for id in (baseline + 1)..=target {
        cluster.write_via_leader(&live, id, id).await;
    }
    cluster
        .wait_indices_have(&live, target as usize, catch_up_timeout)
        .await;

    for &i in &live {
        let _ = cluster.nodes[i].peer.compact_log();
    }

    cluster.restart(follower, &ids, &addrs).await;
    (cluster, follower, target as usize)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "heavy: ~1000 synchronous commits; run with `cargo test -- --ignored`"]
async fn snapshot_install_after_1000_entries() {
    // The full 1000-entry catch-up via snapshot install. Heavy under contended
    // CI parallelism (1000 synchronous majority commits), so ignored by default;
    // `snapshot_install_metrics_increment` and the `preserves_*` tests cover the
    // same path at a CI-safe size.
    let (cluster, follower, target) =
        snapshot_install_scenario_sized(20, 1000, Duration::from_secs(120)).await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(60))
        .await;
    assert!(
        cluster.nodes[follower].peer.snapshot_counters().1 >= 1,
        "follower should have installed a snapshot"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "stress: 10k+ synchronous commits; run with `cargo test -- --ignored`"]
async fn snapshot_install_large_ignored_stress() {
    let (cluster, follower, target) =
        snapshot_install_scenario_sized(50, 10_000, Duration::from_secs(600)).await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(120))
        .await;
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn snapshot_install_metrics_increment() {
    // A CI-safe larger run (well past the baseline) that must catch up by a
    // snapshot install, asserting the v0.6.1 snapshot metrics and diagnostics
    // advance: bytes installed, needs-snapshot detections, and the in-progress
    // gauge settling back to zero once the follower is current.
    let (cluster, follower, target) =
        snapshot_install_scenario_sized(20, 200, Duration::from_secs(30)).await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(30))
        .await;

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let diag = cluster.nodes[follower].peer.snapshot_diagnostics();
        let sent_bytes: u64 = (0..3)
            .map(|i| cluster.nodes[i].peer.snapshot_diagnostics().bytes_sent)
            .sum();
        let needed: u64 = (0..3)
            .map(|i| cluster.nodes[i].peer.peer_metrics().snapshot_needed)
            .sum();
        if diag.bytes_installed > 0 && sent_bytes > 0 && needed >= 1 {
            // The follower recorded a concrete install boundary.
            assert!(
                diag.last_included_index > 0,
                "diagnostics should record the installed boundary"
            );
            break;
        }
        if Instant::now() >= deadline {
            panic!("snapshot metrics did not advance: follower diag={diag:?}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // No duplicate apply on the recovered follower.
    assert_eq!(
        cluster.nodes[follower].peer.metrics().apply_errors,
        0,
        "snapshot install + resume must not produce apply errors"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn snapshot_install_preserves_indexes() {
    let (cluster, follower, target) =
        snapshot_install_scenario_sized(20, 150, Duration::from_secs(30)).await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(30))
        .await;
    // The follower's secondary index on `v` is rebuilt from the installed
    // snapshot: an indexed equality query returns the right record via the index.
    let engine = &cluster.nodes[follower].engine;
    let q = FindQuery {
        filter: Some(Filter::Compare {
            field: "v".into(),
            op: CompareOp::Eq,
            value: auradb::core::Value::Int(75),
        }),
        ..FindQuery::new("C")
    };
    let rows = engine.find(&q).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "indexed lookup should find exactly one record"
    );
    let plan = engine.explain(&q).unwrap();
    assert!(
        plan.used_index.is_some(),
        "the rebuilt index should be used by the planner: {plan:?}"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn snapshot_install_preserves_planner_stats_or_rebuilds() {
    let (cluster, follower, target) =
        snapshot_install_scenario_sized(20, 150, Duration::from_secs(30)).await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(30))
        .await;
    // Planner statistics are rebuilt from the installed snapshot: a range query
    // plans and executes correctly, returning every matching record.
    let engine = &cluster.nodes[follower].engine;
    let q = FindQuery {
        filter: Some(Filter::Compare {
            field: "v".into(),
            op: CompareOp::Lte,
            value: auradb::core::Value::Int(30),
        }),
        ..FindQuery::new("C")
    };
    let plan = engine.explain(&q).unwrap();
    assert!(
        plan.estimated_rows > 0,
        "planner stats should estimate matching rows after install: {plan:?}"
    );
    let rows = engine.find(&q).unwrap();
    assert_eq!(
        rows.len(),
        30,
        "range query should return every record with v <= 30"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn snapshot_install_preserves_mvcc_timestamp_order() {
    let (cluster, follower, target) =
        snapshot_install_scenario_sized(20, 150, Duration::from_secs(30)).await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(30))
        .await;
    // After the install, the follower's applied index equals the snapshot
    // boundary and continues to advance monotonically as post-boundary
    // AppendEntries arrive — the MVCC commit watermark (commit_ts_base + index)
    // never goes backwards across the boundary.
    let all: Vec<usize> = (0..3).collect();
    let before = cluster.nodes[follower].peer.status().applied_index;
    for id in (target as i64 + 1)..=(target as i64 + 15) {
        cluster.write_via_leader(&all, id, id).await;
    }
    cluster
        .wait_indices_have(&[follower], target + 15, Duration::from_secs(30))
        .await;
    let after = cluster.nodes[follower].peer.status().applied_index;
    assert!(
        after > before,
        "applied index must advance monotonically after the snapshot boundary ({before} -> {after})"
    );
    // Every record id is present exactly once (monotonic, gap-free apply).
    let rows = cluster.nodes[follower]
        .engine
        .find(&FindQuery::new("C"))
        .unwrap();
    assert_eq!(rows.len(), target + 15, "no missing or duplicated records");
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn snapshot_install_then_append_entries_resume() {
    let (cluster, follower, target) =
        snapshot_install_scenario_sized(20, 150, Duration::from_secs(30)).await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(30))
        .await;
    // Normal AppendEntries replication resumes after the install.
    let all: Vec<usize> = (0..3).collect();
    for id in (target as i64 + 1)..=(target as i64 + 12) {
        cluster.write_via_leader(&all, id, id).await;
    }
    cluster
        .wait_indices_have(&[follower], target + 12, Duration::from_secs(30))
        .await;
    cluster.shutdown().await;
}

/// Like the sized scenario, but the leader keeps committing writes *after* the
/// follower restarts (concurrent with its snapshot install), then we drive to a
/// final, higher target. Returns the cluster, follower index, and final count.
async fn snapshot_install_concurrent_scenario() -> (TestCluster, usize, usize) {
    let (cluster, follower, target) =
        snapshot_install_scenario_sized(20, 150, Duration::from_secs(30)).await;
    // Commit a further run via the whole cluster while the follower is installing
    // the snapshot and resuming AppendEntries — the two proceed concurrently on
    // the multi-threaded runtime.
    let all: Vec<usize> = (0..3).collect();
    let final_target = target + 60;
    for id in (target as i64 + 1)..=(final_target as i64) {
        cluster.write_via_leader(&all, id, id).await;
    }
    (cluster, follower, final_target)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn snapshot_install_with_concurrent_leader_writes() {
    let (cluster, follower, final_target) = snapshot_install_concurrent_scenario().await;
    cluster
        .wait_indices_have(&[follower], final_target, Duration::from_secs(40))
        .await;
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn snapshot_install_boundary_stable_while_writes_continue() {
    let (cluster, follower, final_target) = snapshot_install_concurrent_scenario().await;
    cluster
        .wait_indices_have(&[follower], final_target, Duration::from_secs(40))
        .await;
    // The follower installed a snapshot at a stable boundary that is consistent
    // with its applied state even though the leader kept writing: the recorded
    // boundary is within the committed range it eventually reached.
    let diag = cluster.nodes[follower].peer.snapshot_diagnostics();
    assert!(
        diag.last_included_index > 0,
        "a snapshot install should have been recorded: {diag:?}"
    );
    let applied = cluster.nodes[follower].peer.status().applied_index;
    assert!(
        diag.last_included_index <= applied,
        "the install boundary ({}) must not exceed the applied index ({applied})",
        diag.last_included_index
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn snapshot_install_no_duplicate_apply_after_concurrent_writes() {
    let (cluster, follower, final_target) = snapshot_install_concurrent_scenario().await;
    cluster
        .wait_indices_have(&[follower], final_target, Duration::from_secs(40))
        .await;
    // Exactly `final_target` distinct records, no duplicates, and no apply errors
    // (an Insert re-applied over the snapshot boundary would conflict and count).
    assert_eq!(
        cluster.record_count(follower),
        final_target,
        "follower must hold each record exactly once"
    );
    assert_eq!(
        cluster.nodes[follower].peer.metrics().apply_errors,
        0,
        "no duplicate/conflicting apply across the snapshot boundary"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn snapshot_install_follower_converges_after_concurrent_writes() {
    let (cluster, follower, final_target) = snapshot_install_concurrent_scenario().await;
    cluster
        .wait_indices_have(&[follower], final_target, Duration::from_secs(40))
        .await;
    // The follower converges to the same record set as the rest of the cluster.
    let live: Vec<usize> = (0..3).collect();
    let max_applied = live
        .iter()
        .map(|&i| cluster.nodes[i].peer.status().applied_index)
        .max()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if cluster.nodes[follower].peer.status().applied_index >= max_applied {
            break;
        }
        if Instant::now() >= deadline {
            panic!("follower did not converge to applied index {max_applied}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cluster.shutdown().await;
}

/// Connect to `addr` and complete the peer handshake as `node_id`, retrying
/// briefly while the listener binds. Returns the open stream.
async fn handshake_as_peer(
    addr: &str,
    cluster_id: ClusterId,
    node_id: NodeId,
) -> tokio::net::TcpStream {
    use auradb_replication::transport::{self, Hello, PeerMessage, MAX_FRAME_BYTES};
    use tokio::net::TcpStream;

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match TcpStream::connect(addr).await {
            Ok(s) => break s,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(e) => panic!("peer listener never came up: {e}"),
        }
    };
    transport::write_message(
        &mut stream,
        &PeerMessage::Hello(Hello {
            cluster_id,
            node_id,
            advertise_addr: "127.0.0.1:1".into(),
            token: auradb_cluster::Secret::default(),
        }),
    )
    .await
    .unwrap();
    match transport::read_message(&mut stream, MAX_FRAME_BYTES)
        .await
        .unwrap()
    {
        PeerMessage::HelloAck(_) => {}
        other => panic!("expected HelloAck, got {other:?}"),
    }
    stream
}

/// A sample snapshot manifest tagged with `cluster_id`, covering boundary
/// index 50 / term 1, with a handful of records.
fn sample_manifest(cluster_id: ClusterId) -> auradb_replication::SnapshotManifest {
    let tmp = tempfile::tempdir().unwrap();
    let src = Engine::open(tmp.path().join("data")).unwrap();
    src.create_schema(schema()).unwrap();
    for id in 1..=5 {
        src.apply_mutation(insert_mutation(id, id)).unwrap();
    }
    auradb_replication::SnapshotManifest::create(&src, 50, 1, "test")
        .unwrap()
        .with_identity(Some(cluster_id.to_string()), None)
}

/// Send an install request to a live node impersonating its configured peer and
/// assert the node rejects it (its rejected-snapshot counter advances).
async fn assert_snapshot_rejected(
    peer: &Arc<PeerCluster>,
    addr: &str,
    cluster_id: ClusterId,
    request: auradb_replication::transport::PeerMessage,
) {
    use auradb_replication::transport;
    let mut stream = handshake_as_peer(addr, cluster_id, NodeId::from_raw(2)).await;
    let before = peer.snapshot_counters().2;
    transport::write_message(&mut stream, &request)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if peer.snapshot_counters().2 > before {
            return;
        }
        if Instant::now() >= deadline {
            panic!("node did not reject the invalid snapshot install");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn install_snapshot_rejects_oversized_payload() {
    use auradb_replication::transport::{PeerMessage, MAX_SNAPSHOT_BYTES};
    let (peer, _dead, addr, cluster_id, _dir, _engine) = node_with_dead_peer().await;
    let req = PeerMessage::InstallSnapshotRequest {
        from: NodeId::from_raw(2),
        term: u64::MAX, // never stale, so the size check is what rejects it
        last_included_index: 50,
        last_included_term: 1,
        snapshot: vec![0u8; MAX_SNAPSHOT_BYTES + 1],
    };
    assert_snapshot_rejected(&peer, &addr, cluster_id, req).await;
    peer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn install_snapshot_rejects_wrong_cluster() {
    use auradb_replication::transport::PeerMessage;
    let (peer, _dead, addr, cluster_id, _dir, _engine) = node_with_dead_peer().await;
    // A manifest from a different cluster id.
    let other = ClusterId::new(0x9999).unwrap();
    let manifest = sample_manifest(other);
    let req = PeerMessage::InstallSnapshotRequest {
        from: NodeId::from_raw(2),
        term: u64::MAX,
        last_included_index: 50,
        last_included_term: 1,
        snapshot: manifest.encode().unwrap(),
    };
    assert_snapshot_rejected(&peer, &addr, cluster_id, req).await;
    peer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn install_snapshot_rejects_bad_digest() {
    use auradb_replication::transport::PeerMessage;
    let (peer, _dead, addr, cluster_id, _dir, _engine) = node_with_dead_peer().await;
    // Same cluster, but the payload is corrupted after the digest was computed.
    let mut manifest = sample_manifest(cluster_id);
    manifest.payload.push(0xFF);
    let req = PeerMessage::InstallSnapshotRequest {
        from: NodeId::from_raw(2),
        term: u64::MAX,
        last_included_index: 50,
        last_included_term: 1,
        snapshot: manifest.encode().unwrap(),
    };
    assert_snapshot_rejected(&peer, &addr, cluster_id, req).await;
    peer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn install_snapshot_rejects_future_format() {
    use auradb_replication::transport::PeerMessage;
    let (peer, _dead, addr, cluster_id, _dir, _engine) = node_with_dead_peer().await;
    let mut manifest = sample_manifest(cluster_id);
    manifest.meta.format_version = 9999; // newer than this build understands
    let req = PeerMessage::InstallSnapshotRequest {
        from: NodeId::from_raw(2),
        term: u64::MAX,
        last_included_index: 50,
        last_included_term: 1,
        snapshot: manifest.encode().unwrap(),
    };
    assert_snapshot_rejected(&peer, &addr, cluster_id, req).await;
    peer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn install_snapshot_failure_preserves_existing_state() {
    use auradb_replication::transport::PeerMessage;
    let (peer, _dead, addr, cluster_id, _dir, engine) = node_with_dead_peer().await;
    // Give the node some existing local state.
    for id in 1..=5 {
        engine.apply_mutation(insert_mutation(id, id)).unwrap();
    }
    assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 5);

    // A corrupt snapshot is rejected before any state is touched.
    let mut manifest = sample_manifest(cluster_id);
    manifest.payload.push(0xFF);
    let req = PeerMessage::InstallSnapshotRequest {
        from: NodeId::from_raw(2),
        term: u64::MAX,
        last_included_index: 50,
        last_included_term: 1,
        snapshot: manifest.encode().unwrap(),
    };
    assert_snapshot_rejected(&peer, &addr, cluster_id, req).await;

    // Existing records are untouched.
    assert_eq!(
        engine.find(&FindQuery::new("C")).unwrap().len(),
        5,
        "a rejected snapshot must not modify existing follower state"
    );
    peer.shutdown().await;
}

// =====================================================================
// v0.6.2: repeated chaos and larger-state recovery hardening.
//
// These build on the same `TestCluster` harness above. They add: repeated
// leader restart / re-election cycles, larger multi-model data-set recovery,
// multi-model snapshot install, a peer reconnect storm, and deterministic
// network-interruption (partition/heal) simulations. As with the rest of the
// file, they assert convergence and safety *outcomes* with bounded polling and
// tolerate intermediate leadership churn rather than depending on a particular
// election race.
// =====================================================================

/// A multi-model collection "C": scalar (`id`, `v`), a secondary-indexed string
/// (`title`), a full-text-indexed body (`body`), a document field with a
/// document-path index (`profile.tag`), and a vector field (`embedding`). It is
/// a drop-in replacement for [`schema`] — the primary key is still `id` and the
/// collection is still "C", so every existing `TestCluster` helper works.
const RICH_DIM: usize = 4;

fn rich_schema() -> CollectionSchema {
    CollectionSchema::new("C")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Int,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef {
            name: "v".into(),
            field_type: FieldType::Int,
            primary_key: false,
            unique: false,
            nullable: false,
            indexed: true,
        })
        .with_field(FieldDef {
            name: "title".into(),
            field_type: FieldType::String,
            primary_key: false,
            unique: false,
            nullable: false,
            indexed: true,
        })
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("profile", FieldType::Document))
        .with_field(FieldDef::new(
            "embedding",
            FieldType::Vector { dim: RICH_DIM },
        ))
        .with_index(IndexDef {
            path: "profile.tag".into(),
            kind: IndexKind::DocumentPath,
        })
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

/// A deterministic multi-model record keyed by `id`: every body contains the
/// shared token `item` (so a full-text query for it matches all records), the
/// document path `profile.tag` buckets into five values, and the embedding is a
/// simple function of `id`.
fn rich_insert_mutation(id: i64) -> Mutation {
    let mut fields = Document::new();
    fields.insert("id".into(), Value::Int(id));
    fields.insert("v".into(), Value::Int(id));
    fields.insert("title".into(), Value::Text(format!("Title {id}")));
    fields.insert(
        "body".into(),
        Value::Text(format!("alpha beta item number {id} gamma delta")),
    );
    let mut profile = Document::new();
    profile.insert("tag".into(), Value::Text(format!("tag{}", id % 5)));
    fields.insert("profile".into(), Value::Object(profile));
    let embedding: Vec<f32> = vec![id as f32, (id % 7) as f32, (id % 3) as f32, 1.0];
    fields.insert("embedding".into(), Value::Vector(embedding));
    Mutation::Insert {
        collection: "C".into(),
        fields,
    }
}

impl TestCluster {
    /// Commit a multi-model record through whichever of `live` currently leads,
    /// retrying on transient conditions. Mirrors [`write_via_leader`] but inserts
    /// a [`rich_insert_mutation`].
    async fn write_rich_via_leader(&self, live: &[usize], id: i64) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut may_have_committed = false;
        loop {
            if let Some(li) = self.live_leader_index(live) {
                let engine = self.nodes[li].engine.clone();
                let result = tokio::task::spawn_blocking(move || {
                    engine.apply_mutation(rich_insert_mutation(id))
                })
                .await
                .unwrap();
                match result {
                    Ok(_) => return,
                    Err(auradb::core::Error::UniqueViolation(_)) if may_have_committed => return,
                    Err(err) => {
                        let transient = matches!(&err, auradb::core::Error::NotLeader(_))
                            || matches!(&err, auradb::core::Error::Internal(m)
                                if m.contains("replication timed out"));
                        if !transient {
                            panic!("rich write {id} via leader {li} failed: {err}");
                        }
                        may_have_committed = true;
                    }
                }
            }
            if Instant::now() >= deadline {
                panic!("could not commit rich write {id} via a leader among {live:?} within 30s");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Simulate a two-way network partition isolating node `idx` from every other
    /// node: each end drops the other's inbound frames. The isolated node keeps
    /// running (its in-memory Raft state is preserved), unlike [`stop`].
    fn partition(&self, idx: usize) {
        for j in 0..self.nodes.len() {
            if j == idx {
                continue;
            }
            self.nodes[idx].peer.drop_peer_link(self.nodes[j].id);
            self.nodes[j].peer.drop_peer_link(self.nodes[idx].id);
        }
    }

    /// Heal the partition created by [`partition`], resuming delivery in both
    /// directions for node `idx`.
    fn heal(&self, idx: usize) {
        for j in 0..self.nodes.len() {
            if j == idx {
                continue;
            }
            self.nodes[idx].peer.heal_peer_link(self.nodes[j].id);
            self.nodes[j].peer.heal_peer_link(self.nodes[idx].id);
        }
    }

    /// Run a multi-model query on node `idx` and return the matching ids.
    fn query_ids(&self, idx: usize, q: &FindQuery) -> std::collections::BTreeSet<i64> {
        self.nodes[idx]
            .engine
            .find(q)
            .unwrap()
            .into_iter()
            .filter_map(|r| match r.fields.get("id") {
                Some(Value::Int(v)) => Some(*v),
                _ => None,
            })
            .collect()
    }

    /// Total `leader_changes` observed by any single node (the maximum across
    /// nodes). A node that restarts resets its own counter, so the cluster-wide
    /// signal that re-elections happened is "some surviving node saw at least
    /// one".
    fn max_leader_changes(&self) -> u64 {
        (0..self.nodes.len())
            .map(|i| self.nodes[i].peer.metrics().leader_changes)
            .max()
            .unwrap_or(0)
    }

    /// Sum of apply errors across all nodes (must stay zero: a duplicate or
    /// conflicting apply would increment it).
    fn total_apply_errors(&self) -> u64 {
        (0..self.nodes.len())
            .map(|i| self.nodes[i].peer.metrics().apply_errors)
            .sum()
    }
}

// ---- Step 3: repeated leader restart and re-election ----

/// Run `cycles` of: commit via the current leader, kill the leader, let the
/// majority elect a new one, commit again through it, then restart the old
/// leader and wait for the whole cluster to reconverge. Returns the cluster and
/// the number of distinct records committed (`2 * cycles`).
async fn run_repeated_leader_restart(cycles: usize) -> (TestCluster, usize) {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    cluster.wait_for_leader(Duration::from_secs(10)).await;

    let mut next_id: i64 = 1;
    for _ in 0..cycles {
        let all: Vec<usize> = (0..3).collect();
        // Commit one record via whoever currently leads, and let it settle on all.
        cluster.write_via_leader(&all, next_id, next_id).await;
        let have = next_id as usize;
        next_id += 1;
        cluster.wait_all_have(have, Duration::from_secs(15)).await;

        // Kill the current leader; the surviving majority elects a new one.
        let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
        cluster.stop(leader).await;
        let live: Vec<usize> = (0..3).filter(|&i| i != leader).collect();
        cluster
            .wait_for_live_leader(&live, Duration::from_secs(10))
            .await;

        // Commit through the new leader while the old leader is down.
        cluster.write_via_leader(&live, next_id, next_id).await;
        let have = next_id as usize;
        next_id += 1;
        cluster
            .wait_indices_have(&live, have, Duration::from_secs(15))
            .await;

        // Bring the old leader back; the full cluster reconverges before the next
        // cycle so each cycle starts from a clean, converged state.
        cluster.restart(leader, &ids, &addrs).await;
        cluster.wait_all_have(have, Duration::from_secs(20)).await;
    }
    (cluster, 2 * cycles)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn repeated_leader_restart_2_cycles_converges() {
    let (cluster, total) = run_repeated_leader_restart(2).await;

    // Every node converged on the identical committed record set.
    let expected: std::collections::BTreeSet<i64> = (1..=total as i64).collect();
    for i in 0..3 {
        let got = cluster.query_ids(i, &FindQuery::new("C"));
        assert_eq!(
            got, expected,
            "node {i} did not converge after repeated leader restarts"
        );
    }

    // The committed data survived every restart (preserves_committed_data) and
    // no record was applied twice (no_duplicate_apply).
    for i in 0..3 {
        assert_eq!(
            cluster.record_count(i),
            total,
            "node {i} record count diverged"
        );
    }
    assert_eq!(
        cluster.total_apply_errors(),
        0,
        "repeated restarts must not produce duplicate/conflicting applies"
    );

    // At least one surviving node observed a leadership change (leader-change
    // metric increments / all_nodes_caught_up).
    assert!(
        cluster.max_leader_changes() >= 1,
        "expected the leader-change metric to increment across re-elections"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "stress: 5 kill/elect/restart cycles; run with `cargo test -- --ignored`"]
async fn repeated_leader_restart_5_cycles_stress() {
    let (cluster, total) = run_repeated_leader_restart(5).await;
    let expected: std::collections::BTreeSet<i64> = (1..=total as i64).collect();
    for i in 0..3 {
        assert_eq!(cluster.query_ids(i, &FindQuery::new("C")), expected);
    }
    assert_eq!(cluster.total_apply_errors(), 0);
    cluster.shutdown().await;
}

// ---- Step 4: larger multi-model data-set recovery ----

/// Bring up a multi-model cluster, commit a `baseline` every node shares, stop a
/// follower, commit up to `target` through the majority, then restart the
/// follower and wait for it to catch up. Returns the cluster, the follower
/// index, and `target`.
async fn run_large_dataset_scenario(baseline: i64, target: i64) -> (TestCluster, usize, usize) {
    let mut cluster = TestCluster::start_with(3, rich_schema()).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    let all: Vec<usize> = (0..3).collect();

    for id in 1..=baseline {
        cluster.write_rich_via_leader(&all, id).await;
    }
    cluster
        .wait_all_have(baseline as usize, Duration::from_secs(30))
        .await;

    let follower = (0..3).find(|&i| i != leader).unwrap();
    cluster.stop(follower).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != follower).collect();
    for id in (baseline + 1)..=target {
        cluster.write_rich_via_leader(&live, id).await;
    }
    cluster
        .wait_indices_have(&live, target as usize, Duration::from_secs(40))
        .await;

    cluster.restart(follower, &ids, &addrs).await;
    cluster
        .wait_indices_have(&[follower], target as usize, Duration::from_secs(40))
        .await;
    (cluster, follower, target as usize)
}

// CI-safe default size. The full "large" run (5,000 records) is the ignored
// `large_dataset_follower_restart_catches_up_5000_stress` below; these required
// tests exercise the identical multi-model catch-up path at a size that keeps
// the serial cluster suite fast on contended runners.
const LARGE_BASELINE: i64 = 20;
const LARGE_TARGET: i64 = 120;

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn large_dataset_follower_restart_catches_up() {
    let (cluster, follower, target) =
        run_large_dataset_scenario(LARGE_BASELINE, LARGE_TARGET).await;
    assert_eq!(
        cluster.record_count(follower),
        target,
        "restarted follower holds every multi-model record"
    );
    // A spot read of the last-written record returns its fields intact.
    let q = FindQuery {
        filter: Some(Filter::Compare {
            field: "id".into(),
            op: CompareOp::Eq,
            value: Value::Int(target as i64),
        }),
        ..FindQuery::new("C")
    };
    let rows = cluster.nodes[follower].engine.find(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("title"),
        Some(&Value::Text(format!("Title {target}"))),
        "spot read returns the catch-up record's fields"
    );
    assert_eq!(cluster.total_apply_errors(), 0);
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn large_dataset_indexes_consistent_after_catchup() {
    let (cluster, follower, _target) =
        run_large_dataset_scenario(LARGE_BASELINE, LARGE_TARGET).await;
    let live = (0..3).find(|&i| i != follower).unwrap();
    // The secondary index on `title` returns the same record on the recovered
    // follower as on a node that never went down, and the planner uses the index.
    let q = FindQuery {
        filter: Some(Filter::Compare {
            field: "title".into(),
            op: CompareOp::Eq,
            value: Value::Text("Title 99".into()),
        }),
        ..FindQuery::new("C")
    };
    assert_eq!(
        cluster.query_ids(follower, &q),
        cluster.query_ids(live, &q),
        "secondary-index lookup must agree across nodes after catch-up"
    );
    assert_eq!(cluster.query_ids(follower, &q).len(), 1);
    let plan = cluster.nodes[follower].engine.explain(&q).unwrap();
    assert!(
        plan.used_index.is_some(),
        "the rebuilt secondary index should be used by the planner: {plan:?}"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn large_dataset_full_text_consistent_after_catchup() {
    let (cluster, follower, target) =
        run_large_dataset_scenario(LARGE_BASELINE, LARGE_TARGET).await;
    let live = (0..3).find(|&i| i != follower).unwrap();
    let q = FindQuery {
        filter: Some(Filter::ContainsText {
            field: "body".into(),
            query: "item".into(),
        }),
        ..FindQuery::new("C")
    };
    let on_follower = cluster.query_ids(follower, &q);
    assert_eq!(
        on_follower,
        cluster.query_ids(live, &q),
        "full-text results must agree across nodes after catch-up"
    );
    assert_eq!(
        on_follower.len(),
        target,
        "every record's body contains the shared token"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn large_dataset_doc_path_consistent_after_catchup() {
    let (cluster, follower, _target) =
        run_large_dataset_scenario(LARGE_BASELINE, LARGE_TARGET).await;
    let live = (0..3).find(|&i| i != follower).unwrap();
    let q = FindQuery {
        filter: Some(Filter::Compare {
            field: "profile.tag".into(),
            op: CompareOp::Eq,
            value: Value::Text("tag2".into()),
        }),
        ..FindQuery::new("C")
    };
    let on_follower = cluster.query_ids(follower, &q);
    assert!(
        !on_follower.is_empty(),
        "document-path query returns records"
    );
    assert_eq!(
        on_follower,
        cluster.query_ids(live, &q),
        "document-path results must agree across nodes after catch-up"
    );
    assert!(
        on_follower.iter().all(|id| id % 5 == 2),
        "every matched record has the queried tag"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn large_dataset_vector_consistent_after_catchup() {
    let (cluster, follower, _target) =
        run_large_dataset_scenario(LARGE_BASELINE, LARGE_TARGET).await;
    let live = (0..3).find(|&i| i != follower).unwrap();
    let q = FindQuery {
        vector: Some(VectorSearch {
            field: "embedding".into(),
            query: vec![10.0, 1.0, 1.0, 1.0],
            k: 5,
            metric: "euclidean".into(),
        }),
        ..FindQuery::new("C")
    };
    let near = cluster.nodes[follower].engine.find(&q).unwrap();
    assert_eq!(near.len(), 5, "nearest-k returns k records");
    assert!(near[0].score.is_some(), "vector search returns scores");
    assert_eq!(
        cluster.query_ids(follower, &q),
        cluster.query_ids(live, &q),
        "vector nearest-k results must agree across nodes after catch-up"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn large_dataset_cluster_restart_preserves_state() {
    let (mut cluster, follower, target) =
        run_large_dataset_scenario(LARGE_BASELINE, LARGE_TARGET).await;
    let _ = follower;
    let ids = cluster.ids();
    let addrs = cluster.addrs();

    // Full cluster restart: stop every node, then bring them all back.
    for i in 0..3 {
        cluster.stop(i).await;
    }
    for i in 0..3 {
        cluster.restart(i, &ids, &addrs).await;
    }
    cluster.wait_for_leader(Duration::from_secs(15)).await;
    cluster.wait_all_have(target, Duration::from_secs(20)).await;

    // Every node still holds the full multi-model state and a full-text query
    // still returns all records.
    let q = FindQuery {
        filter: Some(Filter::ContainsText {
            field: "body".into(),
            query: "item".into(),
        }),
        ..FindQuery::new("C")
    };
    for i in 0..3 {
        assert_eq!(cluster.record_count(i), target, "node {i} lost records");
        assert_eq!(
            cluster.query_ids(i, &q).len(),
            target,
            "node {i} lost full-text index state after restart"
        );
    }
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "stress: 5000 multi-model synchronous commits; run with `cargo test -- --ignored`"]
async fn large_dataset_follower_restart_catches_up_5000_stress() {
    let (cluster, follower, target) = run_large_dataset_scenario(50, 5000).await;
    assert_eq!(cluster.record_count(follower), target);
    assert_eq!(cluster.total_apply_errors(), 0);
    cluster.shutdown().await;
}

// ---- Step 5: multi-model snapshot install ----

/// Like [`run_large_dataset_scenario`] but the live majority compacts its logs
/// past the entries the follower needs, so the follower can only be brought
/// current by a snapshot install (not AppendEntries).
async fn run_rich_snapshot_install_scenario(
    baseline: i64,
    target: i64,
) -> (TestCluster, usize, usize) {
    let mut cluster = TestCluster::start_with(3, rich_schema()).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    let all: Vec<usize> = (0..3).collect();

    for id in 1..=baseline {
        cluster.write_rich_via_leader(&all, id).await;
    }
    cluster
        .wait_all_have(baseline as usize, Duration::from_secs(30))
        .await;

    let follower = (0..3).find(|&i| i != leader).unwrap();
    cluster.stop(follower).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != follower).collect();
    for id in (baseline + 1)..=target {
        cluster.write_rich_via_leader(&live, id).await;
    }
    cluster
        .wait_indices_have(&live, target as usize, Duration::from_secs(40))
        .await;
    for &i in &live {
        let _ = cluster.nodes[i].peer.compact_log();
    }
    cluster.restart(follower, &ids, &addrs).await;
    cluster
        .wait_indices_have(&[follower], target as usize, Duration::from_secs(40))
        .await;
    (cluster, follower, target as usize)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn snapshot_install_preserves_full_text_and_doc_path() {
    let (cluster, follower, target) = run_rich_snapshot_install_scenario(20, 150).await;
    // The install happened (the live nodes compacted the entries it needed). The
    // follower's engine reaches `target` records inside the install handler a hair
    // before the install counter is incremented, so poll the counter with a
    // bounded deadline rather than reading it once right after catch-up.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if cluster.nodes[follower].peer.snapshot_counters().1 >= 1 {
            break;
        }
        if Instant::now() >= deadline {
            panic!("follower should have installed a snapshot");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let live = (0..3).find(|&i| i != follower).unwrap();

    let ft = FindQuery {
        filter: Some(Filter::ContainsText {
            field: "body".into(),
            query: "item".into(),
        }),
        ..FindQuery::new("C")
    };
    assert_eq!(
        cluster.query_ids(follower, &ft).len(),
        target,
        "full-text index rebuilt from the installed snapshot covers all records"
    );
    assert_eq!(
        cluster.query_ids(follower, &ft),
        cluster.query_ids(live, &ft)
    );

    let dp = FindQuery {
        filter: Some(Filter::Compare {
            field: "profile.tag".into(),
            op: CompareOp::Eq,
            value: Value::Text("tag3".into()),
        }),
        ..FindQuery::new("C")
    };
    assert_eq!(
        cluster.query_ids(follower, &dp),
        cluster.query_ids(live, &dp),
        "document-path index must match across nodes after snapshot install"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn snapshot_install_preserves_vector_records() {
    let (cluster, follower, _target) = run_rich_snapshot_install_scenario(20, 150).await;
    let live = (0..3).find(|&i| i != follower).unwrap();
    let q = FindQuery {
        vector: Some(VectorSearch {
            field: "embedding".into(),
            query: vec![12.0, 2.0, 0.0, 1.0],
            k: 5,
            metric: "euclidean".into(),
        }),
        ..FindQuery::new("C")
    };
    let near = cluster.nodes[follower].engine.find(&q).unwrap();
    assert_eq!(near.len(), 5);
    assert!(near[0].score.is_some());
    assert_eq!(
        cluster.query_ids(follower, &q),
        cluster.query_ids(live, &q),
        "vector records must survive a snapshot install intact"
    );
    cluster.shutdown().await;
}

// ---- Step 6: peer reconnect storm ----

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn peer_reconnect_storm_replication_recovers() {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    cluster.write_via_leader(&[0, 1, 2], 1, 1).await;
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    // Repeatedly disconnect and reconnect one follower while the majority keeps
    // committing. The leader must stay stable and replication must resume each
    // time the follower returns.
    let follower = (0..3).find(|&i| i != leader).unwrap();
    let live: Vec<usize> = (0..3).filter(|&i| i != follower).collect();
    let mut next_id: i64 = 2;
    for _ in 0..5 {
        cluster.stop(follower).await;
        cluster.write_via_leader(&live, next_id, next_id).await;
        next_id += 1;
        cluster.restart(follower, &ids, &addrs).await;
        // The follower reconnects and catches up before the next disconnect.
        cluster
            .wait_indices_have(&[follower], (next_id - 1) as usize, Duration::from_secs(15))
            .await;
    }

    let total = (next_id - 1) as usize;
    // Replication fully recovered and the follower is connected again.
    cluster.wait_all_have(total, Duration::from_secs(10)).await;
    let reconnected = cluster.nodes[follower]
        .peer
        .peer_status()
        .iter()
        .any(|s| s.connected);
    assert!(
        reconnected,
        "the follower should hold a live peer connection after the storm"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn peer_reconnect_storm_no_duplicate_apply() {
    let mut cluster = TestCluster::start(3).await;
    let ids = cluster.ids();
    let addrs = cluster.addrs();
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    let follower = (0..3).find(|&i| i != leader).unwrap();
    let live: Vec<usize> = (0..3).filter(|&i| i != follower).collect();

    let mut next_id: i64 = 1;
    for _ in 0..5 {
        cluster.stop(follower).await;
        cluster.write_via_leader(&live, next_id, next_id).await;
        next_id += 1;
        cluster.write_via_leader(&live, next_id, next_id).await;
        next_id += 1;
        cluster.restart(follower, &ids, &addrs).await;
        cluster
            .wait_indices_have(&[follower], (next_id - 1) as usize, Duration::from_secs(15))
            .await;
    }

    let total = (next_id - 1) as usize;
    // Exactly `total` distinct records on every node, no duplicate/conflicting
    // apply despite the repeated reconnect/catch-up cycles.
    for i in 0..3 {
        assert_eq!(cluster.record_count(i), total, "node {i} record count");
    }
    assert_eq!(cluster.total_apply_errors(), 0);
    cluster.shutdown().await;
}

// ---- Step 7: network interruption (partition / heal) simulations ----

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn majority_partition_write_succeeds() {
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    cluster.write_via_leader(&[0, 1, 2], 1, 1).await;
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    // Partition one follower away. The leader and the other follower remain a
    // majority and continue committing.
    let follower = (0..3).find(|&i| i != leader).unwrap();
    cluster.partition(follower);
    let connected: Vec<usize> = (0..3).filter(|&i| i != follower).collect();
    cluster.write_via_leader(&connected, 2, 2).await;
    cluster
        .wait_indices_have(&connected, 2, Duration::from_secs(10))
        .await;

    // Heal the partition; the isolated follower rejoins and converges.
    cluster.heal(follower);
    cluster.wait_for_leader(Duration::from_secs(15)).await;
    cluster.wait_all_have(2, Duration::from_secs(15)).await;
    assert_eq!(cluster.total_apply_errors(), 0);
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn minority_partition_leader_write_times_out() {
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;

    // Isolate the (running) leader from both followers: it is now a minority of
    // one and cannot reach a majority, so a write through it must not commit.
    cluster.partition(leader);
    let write = cluster.write(leader, 9, 9);
    let outcome = tokio::time::timeout(Duration::from_secs(1), write).await;
    assert!(
        outcome.is_err(),
        "a leader partitioned into a minority must not be able to commit"
    );
    assert_eq!(
        cluster.record_count(leader),
        0,
        "the uncommitted write is not visible on the isolated leader"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn partition_heals_and_follower_catches_up() {
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    cluster.write_via_leader(&[0, 1, 2], 1, 1).await;
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    // Drop all traffic to/from one follower (its AppendEntries are dropped),
    // commit a run through the surviving majority, then heal and confirm the
    // follower's log is repaired to the full committed prefix.
    let follower = (0..3).find(|&i| i != leader).unwrap();
    cluster.partition(follower);
    let connected: Vec<usize> = (0..3).filter(|&i| i != follower).collect();
    for id in 2..=12 {
        cluster.write_via_leader(&connected, id, id).await;
    }
    cluster
        .wait_indices_have(&connected, 12, Duration::from_secs(15))
        .await;

    cluster.heal(follower);
    cluster.wait_for_leader(Duration::from_secs(15)).await;
    cluster.wait_all_have(12, Duration::from_secs(20)).await;

    let expected: std::collections::BTreeSet<i64> = (1..=12).collect();
    for i in 0..3 {
        assert_eq!(
            cluster.query_ids(i, &FindQuery::new("C")),
            expected,
            "node {i} did not converge after the partition healed"
        );
    }
    assert_eq!(cluster.total_apply_errors(), 0);
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn leader_partition_triggers_reelection_and_heals() {
    let cluster = TestCluster::start(3).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    cluster.write_via_leader(&[0, 1, 2], 1, 1).await;
    cluster.wait_all_have(1, Duration::from_secs(5)).await;

    // Partition the leader: its heartbeats no longer reach the followers, so the
    // surviving majority must elect a new leader and keep accepting writes.
    cluster.partition(leader);
    let majority: Vec<usize> = (0..3).filter(|&i| i != leader).collect();
    let new_leader = cluster
        .wait_for_live_leader(&majority, Duration::from_secs(15))
        .await;
    assert_ne!(
        new_leader, leader,
        "a new leader takes over the majority side"
    );
    cluster.write_via_leader(&majority, 2, 2).await;

    // Heal: the old leader rejoins, discovers the newer term, and the whole
    // cluster reconverges on a single leader with all records.
    cluster.heal(leader);
    cluster.wait_for_leader(Duration::from_secs(20)).await;
    cluster.wait_all_have(2, Duration::from_secs(20)).await;
    assert!(
        cluster.max_leader_changes() >= 1,
        "the partition should have driven at least one leadership change"
    );
    cluster.shutdown().await;
}

// =====================================================================
// v0.9.0: HA release-candidate hardening.
//
// These build on the same `TestCluster` harness and the existing scenario
// helpers (`run_repeated_leader_restart`, `run_snapshot_install_scenario`,
// `snapshot_install_scenario_sized`, `run_rich_snapshot_install_scenario`,
// `node_with_dead_peer`). They strengthen the repeated fail-stop and
// snapshot/compaction coverage the v0.9.0 HA release candidate is validated
// against (see docs/HA_RELEASE_CANDIDATE.md). The heaviest variants are
// `#[ignore]`d so the default (required) suite stays CI-stable; run them on
// demand with `cargo test -- --ignored`.
// =====================================================================

// ----- repeated fail-stop (longer cycles) -----

/// CI-safe: three kill/elect/rejoin cycles converge with no duplicate apply and
/// at least one observed leadership change.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ha_repeated_leader_restart_3_cycles() {
    let (cluster, total) = run_repeated_leader_restart(3).await;
    let expected: std::collections::BTreeSet<i64> = (1..=total as i64).collect();
    for i in 0..3 {
        assert_eq!(
            cluster.query_ids(i, &FindQuery::new("C")),
            expected,
            "node {i} did not converge after 3 leader restarts"
        );
        assert_eq!(
            cluster.record_count(i),
            total,
            "node {i} record count diverged"
        );
    }
    assert_eq!(
        cluster.total_apply_errors(),
        0,
        "repeated restarts must not produce duplicate/conflicting applies"
    );
    assert!(
        cluster.max_leader_changes() >= 1,
        "expected the leader-change metric to increment across re-elections"
    );
    cluster.shutdown().await;
}

/// Stress: ten kill/elect/rejoin cycles. Heavy under contended CI parallelism
/// (each cycle is several synchronous majority commits plus a restart), so
/// ignored by default; run with `cargo test -- --ignored`.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "stress: 10 kill/elect/restart cycles; run with `cargo test -- --ignored`"]
async fn ha_repeated_leader_restart_10_cycles_ignored() {
    let (cluster, total) = run_repeated_leader_restart(10).await;
    let expected: std::collections::BTreeSet<i64> = (1..=total as i64).collect();
    for i in 0..3 {
        assert_eq!(cluster.query_ids(i, &FindQuery::new("C")), expected);
    }
    assert_eq!(cluster.total_apply_errors(), 0);
    cluster.shutdown().await;
}

/// The old leader rejoins as a follower and is brought current every cycle: the
/// scenario waits for the *whole* cluster (including the just-restarted old
/// leader) to hold every committed record before each next cycle, so a
/// successful run proves per-cycle rejoin and catch-up.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ha_old_leader_rejoins_each_cycle() {
    let (cluster, total) = run_repeated_leader_restart(3).await;
    for i in 0..3 {
        assert_eq!(
            cluster.record_count(i),
            total,
            "node {i} (incl. each rejoined old leader) must hold every committed record"
        );
    }
    assert_eq!(cluster.total_apply_errors(), 0);
    cluster.shutdown().await;
}

/// Repeated restarts never double-apply: every node holds each record exactly
/// once and the apply-error counter stays zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ha_repeated_restart_no_duplicate_apply() {
    let (cluster, total) = run_repeated_leader_restart(3).await;
    for i in 0..3 {
        assert_eq!(
            cluster.record_count(i),
            total,
            "node {i} holds a duplicated or missing record"
        );
    }
    assert_eq!(
        cluster.total_apply_errors(),
        0,
        "an Insert re-applied after a restart would conflict and increment apply_errors"
    );
    cluster.shutdown().await;
}

/// After repeated restarts the cluster's applied indices converge: every node
/// reaches the maximum applied index (no node is left permanently behind).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ha_repeated_restart_indices_converge() {
    let (cluster, _total) = run_repeated_leader_restart(3).await;
    let max_applied = (0..3)
        .map(|i| cluster.nodes[i].peer.status().applied_index)
        .max()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if (0..3).all(|i| cluster.nodes[i].peer.status().applied_index >= max_applied) {
            break;
        }
        if Instant::now() >= deadline {
            let applied: Vec<u64> = (0..3)
                .map(|i| cluster.nodes[i].peer.status().applied_index)
                .collect();
            panic!("applied indices did not converge to {max_applied}: {applied:?}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cluster.shutdown().await;
}

// ----- snapshot install and compaction (larger / offline follower) -----

/// An offline follower behind the leader's *compacted* prefix is brought current
/// by a snapshot install and converges on the full committed record set.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn ha_snapshot_install_after_compaction_with_offline_follower() {
    let (cluster, follower, target) = run_snapshot_install_scenario().await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(30))
        .await;
    // The only possible catch-up path was a snapshot install (both live nodes
    // compacted the entries the follower needed). Poll the counters: the engine
    // reaches `target` a hair before the install counter increments.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let installed = cluster.nodes[follower].peer.snapshot_counters().1;
        let sent: u64 = (0..3)
            .map(|i| cluster.nodes[i].peer.snapshot_counters().0)
            .sum();
        if installed >= 1 && sent >= 1 {
            break;
        }
        if Instant::now() >= deadline {
            panic!("expected a snapshot install: installed={installed}, sent={sent}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        cluster.record_count(follower),
        target,
        "the recovered follower holds the full committed set"
    );
    cluster.shutdown().await;
}

/// After a snapshot install, further writes committed by the whole cluster reach
/// the recovered follower (AppendEntries resumes and it converges).
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn ha_snapshot_install_then_more_writes_converges() {
    let (cluster, follower, target) =
        snapshot_install_scenario_sized(20, 150, Duration::from_secs(30)).await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(30))
        .await;
    let all: Vec<usize> = (0..3).collect();
    for id in (target as i64 + 1)..=(target as i64 + 25) {
        cluster.write_via_leader(&all, id, id).await;
    }
    cluster
        .wait_indices_have(&[follower], target + 25, Duration::from_secs(30))
        .await;
    assert_eq!(cluster.record_count(follower), target + 25);
    cluster.shutdown().await;
}

/// A snapshot install rebuilds the follower's full indexed workload: secondary
/// index, full-text, document-path, and vector queries all match the live nodes.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn ha_snapshot_install_preserves_indexed_workload() {
    let (cluster, follower, target) = run_rich_snapshot_install_scenario(20, 150).await;
    let live = (0..3).find(|&i| i != follower).unwrap();

    // Full-text: every record body carries the shared token "item".
    let ft = FindQuery {
        filter: Some(Filter::ContainsText {
            field: "body".into(),
            query: "item".into(),
        }),
        ..FindQuery::new("C")
    };
    assert_eq!(cluster.query_ids(follower, &ft).len(), target);
    assert_eq!(
        cluster.query_ids(follower, &ft),
        cluster.query_ids(live, &ft)
    );

    // Document-path index on profile.tag.
    let dp = FindQuery {
        filter: Some(Filter::Compare {
            field: "profile.tag".into(),
            op: CompareOp::Eq,
            value: Value::Text("tag3".into()),
        }),
        ..FindQuery::new("C")
    };
    assert_eq!(
        cluster.query_ids(follower, &dp),
        cluster.query_ids(live, &dp)
    );

    // Secondary index on `v` (rich records set v == id).
    let sec = FindQuery {
        filter: Some(Filter::Compare {
            field: "v".into(),
            op: CompareOp::Eq,
            value: Value::Int(75),
        }),
        ..FindQuery::new("C")
    };
    assert_eq!(
        cluster.query_ids(follower, &sec),
        cluster.query_ids(live, &sec)
    );

    // Vector search.
    let vq = FindQuery {
        vector: Some(VectorSearch {
            field: "embedding".into(),
            query: vec![12.0, 2.0, 0.0, 1.0],
            k: 5,
            metric: "euclidean".into(),
        }),
        ..FindQuery::new("C")
    };
    assert_eq!(
        cluster.query_ids(follower, &vq),
        cluster.query_ids(live, &vq)
    );
    cluster.shutdown().await;
}

/// Compaction while every follower is caught up is safe: it forces no snapshot
/// install, and replication keeps flowing via AppendEntries afterward.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn ha_compaction_with_all_followers_caught_up() {
    let cluster = TestCluster::start(3).await;
    cluster.wait_for_leader(Duration::from_secs(10)).await;
    let all: Vec<usize> = (0..3).collect();

    for id in 1..=40 {
        cluster.write_via_leader(&all, id, id).await;
    }
    cluster.wait_all_have(40, Duration::from_secs(20)).await;

    // Everyone is caught up; compacting now drops only already-applied entries.
    for i in 0..3 {
        let _ = cluster.nodes[i].peer.compact_log();
    }

    // Replication still flows via AppendEntries (no snapshot needed).
    for id in 41..=60 {
        cluster.write_via_leader(&all, id, id).await;
    }
    cluster.wait_all_have(60, Duration::from_secs(20)).await;

    let installed: u64 = (0..3)
        .map(|i| cluster.nodes[i].peer.snapshot_counters().1)
        .sum();
    assert_eq!(
        installed, 0,
        "compaction with all followers caught up must not force a snapshot install"
    );
    assert_eq!(cluster.total_apply_errors(), 0);
    cluster.shutdown().await;
}

/// Compaction while one follower is offline forces that follower onto the
/// snapshot-install path on restart (the needs-snapshot detection fires and an
/// install is recorded).
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn ha_compaction_with_offline_follower_requires_snapshot() {
    let (cluster, follower, target) = run_snapshot_install_scenario().await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(30))
        .await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let needed: u64 = (0..3)
            .map(|i| cluster.nodes[i].peer.peer_metrics().snapshot_needed)
            .sum();
        let installed = cluster.nodes[follower].peer.snapshot_counters().1;
        if needed >= 1 && installed >= 1 {
            break;
        }
        if Instant::now() >= deadline {
            panic!("expected needs-snapshot detection and an install after compaction");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cluster.shutdown().await;
}

/// A failed snapshot install is safe to retry: a corrupt install is rejected
/// without touching existing follower state, no install is recorded, and
/// re-sending is rejected the same way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ha_snapshot_failure_safe_to_retry() {
    use auradb_replication::transport::PeerMessage;
    let (peer, _dead, addr, cluster_id, _dir, engine) = node_with_dead_peer().await;
    for id in 1..=5 {
        engine.apply_mutation(insert_mutation(id, id)).unwrap();
    }
    assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 5);

    let corrupt = || {
        let mut manifest = sample_manifest(cluster_id);
        manifest.payload.push(0xFF);
        PeerMessage::InstallSnapshotRequest {
            from: NodeId::from_raw(2),
            term: u64::MAX,
            last_included_index: 50,
            last_included_term: 1,
            snapshot: manifest.encode().unwrap(),
        }
    };

    // First attempt is rejected; existing state is preserved and nothing is
    // recorded as installed.
    assert_snapshot_rejected(&peer, &addr, cluster_id, corrupt()).await;
    assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 5);
    assert_eq!(
        peer.snapshot_counters().1,
        0,
        "a rejected install must not be recorded as installed"
    );

    // Retrying is safe: rejected again the same way, state still intact.
    assert_snapshot_rejected(&peer, &addr, cluster_id, corrupt()).await;
    assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 5);
    assert_eq!(peer.snapshot_counters().1, 0);
    peer.shutdown().await;
}

/// Snapshot install metrics reflect the transfer: bytes installed on the
/// follower, bytes sent by a live node, the needs-snapshot detection, and a
/// recorded install boundary — with no apply errors.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn ha_snapshot_metrics_after_install() {
    let (cluster, follower, target) =
        snapshot_install_scenario_sized(20, 200, Duration::from_secs(30)).await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(30))
        .await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let diag = cluster.nodes[follower].peer.snapshot_diagnostics();
        let sent_bytes: u64 = (0..3)
            .map(|i| cluster.nodes[i].peer.snapshot_diagnostics().bytes_sent)
            .sum();
        let needed: u64 = (0..3)
            .map(|i| cluster.nodes[i].peer.peer_metrics().snapshot_needed)
            .sum();
        if diag.bytes_installed > 0 && sent_bytes > 0 && needed >= 1 {
            assert!(
                diag.last_included_index > 0,
                "diagnostics should record the installed boundary"
            );
            break;
        }
        if Instant::now() >= deadline {
            panic!("snapshot metrics did not advance: {diag:?}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        cluster.nodes[follower].peer.metrics().apply_errors,
        0,
        "snapshot install + resume must not produce apply errors"
    );
    cluster.shutdown().await;
}

/// Stress: a larger snapshot install (more committed entries before the
/// follower catches up via a single snapshot). Heavy, so ignored by default;
/// run with `cargo test -- --ignored`.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "stress: larger snapshot install; run with `cargo test -- --ignored`"]
async fn ha_snapshot_large_ignored_stress() {
    let (cluster, follower, target) =
        snapshot_install_scenario_sized(50, 3000, Duration::from_secs(300)).await;
    cluster
        .wait_indices_have(&[follower], target, Duration::from_secs(120))
        .await;
    assert!(
        cluster.nodes[follower].peer.snapshot_counters().1 >= 1,
        "follower should have installed a snapshot"
    );
    cluster.shutdown().await;
}

// ----- cluster backup / restore around leader change (v0.9.0) -----
//
// The supported cluster backup story is a *leader logical export -> single-node
// restore* path (see docs/HA_RELEASE_CANDIDATE.md and docs/OPERATIONS.md). These
// tests validate it around a leader change: a backup is taken from the leader,
// the leader is killed, the new leader takes more writes, a fresh backup from
// the new leader captures the latest committed state, and that backup restores
// into a fresh single node — which can then bootstrap its own one-node cluster.
//
// The export/restore here is the same logical operation as `auradb dump` /
// `auradb restore` (schemas + records as JSONL, restored as upserts), exercised
// directly against the engine to avoid a crate dependency cycle.

/// One line of a logical backup, mirroring `auradb dump`'s JSONL format.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BackupLine {
    Schema {
        schema: CollectionSchema,
    },
    Record {
        collection: String,
        fields: Document,
    },
}

impl TestCluster {
    /// Take a logical backup of node `idx`'s engine to `out` (every schema, then
    /// every record, as JSONL) — the "back up from the leader" operation.
    /// Returns the record count. Mirrors `auradb dump`.
    fn backup_node(&self, idx: usize, out: &std::path::Path) -> usize {
        let engine = &self.nodes[idx].engine;
        let mut buf = String::new();
        let schemas = engine.list_schemas();
        for schema in &schemas {
            let line = BackupLine::Schema {
                schema: schema.clone(),
            };
            buf.push_str(&serde_json::to_string(&line).unwrap());
            buf.push('\n');
        }
        let mut count = 0;
        for schema in &schemas {
            for row in engine.find(&FindQuery::new(&schema.name)).unwrap() {
                let line = BackupLine::Record {
                    collection: schema.name.clone(),
                    fields: row.fields,
                };
                buf.push_str(&serde_json::to_string(&line).unwrap());
                buf.push('\n');
                count += 1;
            }
        }
        std::fs::write(out, buf).unwrap();
        count
    }
}

/// Restore a logical backup into a fresh single-node engine at `data_dir`
/// (create each schema, upsert each record). Returns the engine and the record
/// count. Mirrors `auradb restore`.
fn restore_to_single_node(backup: &std::path::Path, data_dir: &std::path::Path) -> (Engine, usize) {
    let engine = Engine::open(data_dir).unwrap();
    let content = std::fs::read_to_string(backup).unwrap();
    let mut records = 0;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<BackupLine>(line).unwrap() {
            BackupLine::Schema { schema } => {
                engine.create_schema(schema).unwrap();
            }
            BackupLine::Record { collection, fields } => {
                engine
                    .apply_mutation(Mutation::Upsert { collection, fields })
                    .unwrap();
                records += 1;
            }
        }
    }
    (engine, records)
}

/// The set of `id` values in collection "C" on an engine.
fn engine_ids(engine: &Engine) -> std::collections::BTreeSet<i64> {
    engine
        .find(&FindQuery::new("C"))
        .unwrap()
        .into_iter()
        .filter_map(|r| match r.fields.get("id") {
            Some(Value::Int(v)) => Some(*v),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cluster_backup_before_and_after_leader_change() {
    let cluster = TestCluster::start(3).await;
    cluster.wait_for_leader(Duration::from_secs(10)).await;

    for id in 1..=10 {
        cluster.write_via_leader(&[0, 1, 2], id, id).await;
    }
    cluster.wait_all_have(10, Duration::from_secs(15)).await;

    // Back up from the current leader before the failure.
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    let dir = tempfile::tempdir().unwrap();
    let backup_before = dir.path().join("before.jsonl");
    assert_eq!(
        cluster.backup_node(leader, &backup_before),
        10,
        "pre-failure backup captures the committed baseline"
    );

    // Kill the leader; the majority elects a new one and accepts more writes.
    cluster.stop(leader).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != leader).collect();
    let new_leader = cluster
        .wait_for_live_leader(&live, Duration::from_secs(10))
        .await;
    for id in 11..=20 {
        cluster.write_via_leader(&live, id, id).await;
    }
    cluster
        .wait_indices_have(&live, 20, Duration::from_secs(15))
        .await;

    // Back up from the NEW leader: it carries every committed record.
    let backup_after = dir.path().join("after.jsonl");
    assert_eq!(
        cluster.backup_node(new_leader, &backup_after),
        20,
        "post-change backup from the new leader captures the latest committed state"
    );
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cluster_backup_restore_latest_leader_state() {
    let cluster = TestCluster::start(3).await;
    cluster.wait_for_leader(Duration::from_secs(10)).await;
    for id in 1..=10 {
        cluster.write_via_leader(&[0, 1, 2], id, id).await;
    }
    cluster.wait_all_have(10, Duration::from_secs(15)).await;

    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    cluster.stop(leader).await;
    let live: Vec<usize> = (0..3).filter(|&i| i != leader).collect();
    let new_leader = cluster
        .wait_for_live_leader(&live, Duration::from_secs(10))
        .await;
    for id in 11..=20 {
        cluster.write_via_leader(&live, id, id).await;
    }
    cluster
        .wait_indices_have(&live, 20, Duration::from_secs(15))
        .await;

    // Back up the new leader and restore into a fresh single node.
    let dir = tempfile::tempdir().unwrap();
    let backup = dir.path().join("latest.jsonl");
    assert_eq!(cluster.backup_node(new_leader, &backup), 20);

    let restored_dir = tempfile::tempdir().unwrap();
    let (engine, restored) = restore_to_single_node(&backup, &restored_dir.path().join("data"));
    assert_eq!(restored, 20, "restore loads every committed record");
    assert_eq!(
        engine_ids(&engine),
        (1..=20).collect(),
        "the restored single node carries the latest committed state"
    );
    drop(engine);
    cluster.shutdown().await;
}

/// Restore targets a fresh, offline single-node data directory — there is no
/// operation to restore into a live multi-node cluster. `restore` opens a local
/// data dir and upserts; it never contacts a peer or a running node, so a
/// restored node is independent of (and cannot disturb) the live cluster.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cluster_restore_live_cluster_rejected_or_documented() {
    let cluster = TestCluster::start(3).await;
    cluster.wait_for_leader(Duration::from_secs(10)).await;
    for id in 1..=8 {
        cluster.write_via_leader(&[0, 1, 2], id, id).await;
    }
    cluster.wait_all_have(8, Duration::from_secs(15)).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;

    let dir = tempfile::tempdir().unwrap();
    let backup = dir.path().join("b.jsonl");
    assert_eq!(cluster.backup_node(leader, &backup), 8);

    // The only restore path is into a fresh, offline single-node data directory.
    let restored_dir = tempfile::tempdir().unwrap();
    let (engine, restored) = restore_to_single_node(&backup, &restored_dir.path().join("data"));
    assert_eq!(restored, 8);
    assert_eq!(engine_ids(&engine), (1..=8).collect());
    drop(engine);

    // The live cluster is untouched by the restore: every node still holds
    // exactly the 8 committed records.
    for i in 0..3 {
        assert_eq!(
            cluster.record_count(i),
            8,
            "restoring to a separate single node must not affect the live cluster"
        );
    }
    cluster.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cluster_restore_to_single_node_then_bootstrap_preview_cluster() {
    // Take a backup from a live cluster leader, then tear the cluster down.
    let cluster = TestCluster::start(3).await;
    cluster.wait_for_leader(Duration::from_secs(10)).await;
    for id in 1..=12 {
        cluster.write_via_leader(&[0, 1, 2], id, id).await;
    }
    cluster.wait_all_have(12, Duration::from_secs(15)).await;
    let leader = cluster.wait_for_leader(Duration::from_secs(10)).await;
    let dir = tempfile::tempdir().unwrap();
    let backup = dir.path().join("b.jsonl");
    assert_eq!(cluster.backup_node(leader, &backup), 12);
    cluster.shutdown().await;

    // Restore into a fresh single-node data dir.
    let restored_dir = tempfile::tempdir().unwrap();
    let data_path = restored_dir.path().join("data");
    let (engine, restored) = restore_to_single_node(&backup, &data_path);
    assert_eq!(restored, 12);
    drop(engine);

    // Bootstrap a single-node cluster around the restored data dir: it is its own
    // majority, elects itself leader, and serves the restored state.
    let cluster_id = ClusterId::new(0xC0FFEE).unwrap();
    let port = reserve_ports(1)[0];
    let addr = format!("127.0.0.1:{port}");
    let node_id = NodeId::from_raw(1);
    let engine = Engine::open(&data_path).unwrap();
    let cfg = node_config(&[node_id], &[addr], 0, cluster_id);
    let id = identity(cluster_id, node_id);
    let peer =
        PeerCluster::spawn(engine.clone(), id, cfg, restored_dir.path().join("cluster")).unwrap();
    engine.attach_replicated_log(peer.write_log());

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if peer.is_leader() {
            break;
        }
        if Instant::now() >= deadline {
            panic!("bootstrapped single-node cluster did not elect itself leader");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        engine_ids(&engine),
        (1..=12).collect(),
        "the rebuilt single-node cluster serves the restored state"
    );

    // A new write through the rebuilt node commits through its own Raft log.
    // Retry transient NotLeader / commit-timeout conditions while leadership
    // settles on a freshly bootstrapped node.
    let write_deadline = Instant::now() + Duration::from_secs(15);
    let mut may_have_committed = false;
    loop {
        let e2 = engine.clone();
        let result =
            tokio::task::spawn_blocking(move || e2.apply_mutation(insert_mutation(13, 13)))
                .await
                .unwrap();
        match result {
            Ok(_) => break,
            // A prior attempt may have committed but lost its ack (transient
            // churn on a freshly bootstrapped node); re-observing it as a
            // primary-key conflict means the write did land.
            Err(auradb::core::Error::UniqueViolation(_)) if may_have_committed => break,
            Err(err) => {
                let transient = matches!(&err, auradb::core::Error::NotLeader(_))
                    || matches!(&err, auradb::core::Error::Internal(m)
                        if m.contains("replication timed out"));
                if !transient || Instant::now() >= write_deadline {
                    panic!("write through rebuilt single-node cluster failed: {err}");
                }
                may_have_committed = true;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    assert_eq!(engine_ids(&engine), (1..=13).collect());
    peer.shutdown().await;
}
