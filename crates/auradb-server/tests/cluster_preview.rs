//! End-to-end multi-node preview tests over the Aura Wire Protocol.
//!
//! These start three real `Server` processes (in-process tasks bound to real
//! loopback sockets) configured for the experimental multi-node preview, elect a
//! leader across them, and verify client routing: a write to the leader succeeds,
//! a write to a follower comes back as `not_leader` with a leader hint, and a
//! follower's health reports its role and the recognized leader.
//!
//! Readiness is established by polling cluster status — no fixed "wait for the
//! cluster to settle" sleeps.

use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};
use auradb::query::Mutation;
use auradb_cluster::{ClusterConfig, NodeId, NodeRole, PeerConfig};
use auradb_protocol::{Frame, HealthReport, Opcode, RequestId, DEFAULT_MAX_PAYLOAD};
use auradb_server::{read_frame, write_frame, Config, Server};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

fn reserve_ports(n: usize) -> Vec<u16> {
    let mut held = Vec::new();
    let mut ports = Vec::new();
    for _ in 0..n {
        let l = StdTcpListener::bind("127.0.0.1:0").unwrap();
        ports.push(l.local_addr().unwrap().port());
        held.push(l);
    }
    ports
}

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

struct RunningNode {
    server: Arc<Server>,
    client_addr: String,
    shutdown: Arc<Notify>,
    _dir: tempfile::TempDir,
}

/// Start a three-node preview cluster; return the running nodes plus the shared
/// cluster id (hex). The client listeners are bound here and handed to `run_on`.
async fn start_cluster() -> Vec<RunningNode> {
    let n = 3;
    let client_ports = reserve_ports(n);
    let cluster_ports = reserve_ports(n);
    let cluster_id = "00000000000000000000000000abcdef";
    let ids: Vec<NodeId> = (1..=n as u64).map(NodeId::from_raw).collect();
    let cluster_addrs: Vec<String> = cluster_ports
        .iter()
        .map(|p| format!("127.0.0.1:{p}"))
        .collect();

    let mut nodes = Vec::new();
    for i in 0..n {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_path_buf();

        let peers: Vec<PeerConfig> = (0..n)
            .filter(|&j| j != i)
            .map(|j| PeerConfig {
                node_id: ids[j].to_string(),
                addr: cluster_addrs[j].clone(),
                // Declare each peer's client-facing address so a `not_leader`
                // response and the cluster health can report the leader's client
                // address (the v0.5.1 ergonomics).
                client_addr: Some(format!("127.0.0.1:{}", client_ports[j])),
            })
            .collect();
        let cluster = ClusterConfig {
            enabled: true,
            experimental_multi_node: true,
            cluster_id: cluster_id.to_string(),
            node_id: ids[i].to_string(),
            listen_addr: cluster_addrs[i].clone(),
            advertise_addr: cluster_addrs[i].clone(),
            // Declare this node's own client address so it can report it as the
            // leader hint while it leads (the v0.9.1 leader self-report).
            advertise_client_addr: Some(format!("127.0.0.1:{}", client_ports[i])),
            bootstrap: true,
            peers,
            ..ClusterConfig::default()
        };
        let config = Config {
            bind: "127.0.0.1".into(),
            port: client_ports[i],
            data_dir,
            cluster,
            ..Config::default()
        };

        let listener = TcpListener::bind(("127.0.0.1", client_ports[i]))
            .await
            .unwrap();
        let client_addr = listener.local_addr().unwrap().to_string();
        let server = Arc::new(Server::open(config).unwrap());
        // Seed the schema on every node so a replicated insert is valid.
        server.context().engine.create_schema(schema()).unwrap();

        let shutdown = Arc::new(Notify::new());
        let s2 = Arc::clone(&server);
        let sd = Arc::clone(&shutdown);
        tokio::spawn(async move {
            let _ = s2
                .run_on(listener, async move { sd.notified().await })
                .await;
        });
        nodes.push(RunningNode {
            server,
            client_addr,
            shutdown,
            _dir: dir,
        });
    }
    nodes
}

/// Poll cluster status until exactly one node is leader and a majority recognize
/// it; return that node's index.
async fn wait_for_leader(nodes: &[RunningNode], timeout: Duration) -> usize {
    let deadline = Instant::now() + timeout;
    loop {
        let statuses: Vec<_> = nodes
            .iter()
            .map(|n| n.server.cluster_status().unwrap())
            .collect();
        let leaders: Vec<usize> = statuses
            .iter()
            .enumerate()
            .filter(|(_, s)| s.role == NodeRole::Leader)
            .map(|(i, _)| i)
            .collect();
        if leaders.len() == 1 {
            let leader_id = statuses[leaders[0]].node_id;
            let recognizers = statuses.iter().filter(|s| s.leader_id == leader_id).count();
            if recognizers > nodes.len() / 2 {
                return leaders[0];
            }
        }
        if Instant::now() >= deadline {
            panic!("no leader elected within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn hello(stream: &mut TcpStream) {
    let req = Frame::json(
        Opcode::Hello,
        RequestId(1),
        0,
        &serde_json::json!({ "client_version": "test", "protocol_version": 1 }),
    )
    .unwrap();
    write_frame(stream, &req).await.unwrap();
    read_frame(stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
}

fn insert_frame(req_id: u128, id: i64) -> Frame {
    let mut fields = Document::new();
    fields.insert("id".into(), Value::Int(id));
    fields.insert("v".into(), Value::Int(id * 10));
    let mutation = Mutation::Insert {
        collection: "C".into(),
        fields,
    };
    Frame::json(Opcode::Mutate, RequestId(req_id), 0, &mutation).unwrap()
}

async fn awp_insert(addr: &str, req_id: u128, id: i64) -> Frame {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    hello(&mut stream).await;
    write_frame(&mut stream, &insert_frame(req_id, id))
        .await
        .unwrap();
    read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap()
}

async fn awp_health(addr: &str) -> HealthReport {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    hello(&mut stream).await;
    let req = Frame::new(Opcode::Health, RequestId(9), 0, Vec::new());
    write_frame(&mut stream, &req).await.unwrap();
    let resp = read_frame(&mut stream, DEFAULT_MAX_PAYLOAD)
        .await
        .unwrap()
        .unwrap();
    resp.decode_json().unwrap()
}

fn shutdown_all(nodes: &[RunningNode]) {
    for n in nodes {
        n.shutdown.notify_one();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_server_cluster_elects_leader_and_routes_writes() {
    let nodes = start_cluster().await;
    let leader = wait_for_leader(&nodes, Duration::from_secs(15)).await;

    // A write to the leader succeeds (it commits across a majority before the
    // server returns).
    let resp = awp_insert(&nodes[leader].client_addr, 2, 1).await;
    if resp.opcode != Opcode::Ok {
        let payload: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
        panic!(
            "leader write failed: {:?} {}",
            payload.code, payload.message
        );
    }

    // A write to a follower is refused with a structured not_leader error.
    let follower = (0..nodes.len()).find(|&i| i != leader).unwrap();
    let resp = awp_insert(&nodes[follower].client_addr, 3, 2).await;
    assert_eq!(resp.opcode, Opcode::Error, "follower must refuse the write");
    let payload: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, auradb_core::ErrorCode::NotLeader);
    assert!(
        payload.message.contains("not the leader"),
        "not_leader carries a leader hint: {}",
        payload.message
    );

    shutdown_all(&nodes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn follower_health_reports_role_and_leader() {
    let nodes = start_cluster().await;
    let leader = wait_for_leader(&nodes, Duration::from_secs(15)).await;
    let follower = (0..nodes.len()).find(|&i| i != leader).unwrap();

    let health = awp_health(&nodes[follower].client_addr).await;
    let cluster = health.cluster.expect("cluster health present");
    assert_eq!(cluster.role, "follower");
    assert!(cluster.enabled);
    assert!(!cluster.single_node);
    // The follower recognizes the elected leader.
    let leader_id = nodes[leader].server.cluster_status().unwrap().node_id;
    assert_eq!(cluster.leader_id, leader_id.map(|id| id.to_string()));

    shutdown_all(&nodes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_leader_error_contains_leader_client_addr() {
    let nodes = start_cluster().await;
    let leader = wait_for_leader(&nodes, Duration::from_secs(15)).await;
    let follower = (0..nodes.len()).find(|&i| i != leader).unwrap();

    // A write to a follower returns not_leader; because each node declared its
    // peers' client addresses, the message names the leader's client address.
    let resp = awp_insert(&nodes[follower].client_addr, 3, 2).await;
    assert_eq!(resp.opcode, Opcode::Error);
    let payload: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, auradb_core::ErrorCode::NotLeader);
    assert_eq!(payload.retryable, Some(true), "not_leader is retryable");
    let leader_client = &nodes[leader].client_addr;
    assert!(
        payload.message.contains(leader_client),
        "not_leader names the leader's client address {leader_client}: {}",
        payload.message
    );

    shutdown_all(&nodes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_leader_payload_includes_leader_client_addr_when_known() {
    // The structured not_leader payload carries the leader's client address (and a
    // usable `leader_hint`), the leader/current node ids, role, and term — so a
    // connector can redirect without parsing the human message.
    let nodes = start_cluster().await;
    let leader = wait_for_leader(&nodes, Duration::from_secs(15)).await;
    let follower = (0..nodes.len()).find(|&i| i != leader).unwrap();

    let resp = awp_insert(&nodes[follower].client_addr, 3, 2).await;
    assert_eq!(resp.opcode, Opcode::Error);
    let payload: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, auradb_core::ErrorCode::NotLeader);
    assert_eq!(
        payload.retryable,
        Some(true),
        "retryable true when leader known"
    );

    let details = payload
        .not_leader
        .expect("structured not_leader hints present");
    let leader_client = nodes[leader].client_addr.as_str();
    assert_eq!(details.leader_client_addr.as_deref(), Some(leader_client));
    assert_eq!(details.leader_hint.as_deref(), Some(leader_client));
    assert_eq!(details.role, "follower");

    let leader_id = nodes[leader].server.cluster_status().unwrap().node_id;
    let follower_id = nodes[follower].server.cluster_status().unwrap().node_id;
    assert_eq!(details.leader_node_id, leader_id.map(|id| id.to_string()));
    assert_eq!(
        details.current_node_id,
        follower_id.map(|id| id.to_string())
    );

    shutdown_all(&nodes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_leader_payload_contains_no_secrets() {
    // The structured payload over a live cluster carries only routing metadata,
    // never credential material.
    let nodes = start_cluster().await;
    let leader = wait_for_leader(&nodes, Duration::from_secs(15)).await;
    let follower = (0..nodes.len()).find(|&i| i != leader).unwrap();

    let resp = awp_insert(&nodes[follower].client_addr, 3, 2).await;
    let json = serde_json::to_string(&resp.decode_json::<auradb_protocol::ErrorPayload>().unwrap())
        .unwrap()
        .to_lowercase();
    for needle in ["token", "password", "secret", "bearer"] {
        assert!(
            !json.contains(needle),
            "payload must not contain {needle:?}"
        );
    }

    shutdown_all(&nodes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_status_reports_leader_client_addr() {
    let nodes = start_cluster().await;
    let leader = wait_for_leader(&nodes, Duration::from_secs(15)).await;
    let follower = (0..nodes.len()).find(|&i| i != leader).unwrap();

    // The follower's health reports the leader's declared client address.
    let health = awp_health(&nodes[follower].client_addr).await;
    let cluster = health.cluster.expect("cluster health present");
    assert_eq!(
        cluster.leader_client_addr.as_deref(),
        Some(nodes[leader].client_addr.as_str()),
        "health reports the leader's client address"
    );
    // Per-peer diagnostics carry each peer's declared client address.
    assert!(
        cluster.peers.iter().all(|p| p.client_addr.is_some()),
        "every peer reports a declared client address"
    );

    shutdown_all(&nodes);
}

#[test]
fn docker_compose_cluster_not_leader_hint_has_client_addr_if_configured() {
    // The shipped Compose cluster configs declare each node's own
    // advertise_client_addr and every peer's client_addr, so an *in-network*
    // client's not_leader hint carries the leader's client address. (That address
    // is the in-Docker-network name, e.g. node2:7171, not the host-published port;
    // a host client therefore falls back to re-resolving, which is documented.)
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    for (file, me, peers) in [
        (
            "examples/cluster/docker/node1.toml",
            "node1:7171",
            ["node2:7171", "node3:7171"],
        ),
        (
            "examples/cluster/docker/node2.toml",
            "node2:7171",
            ["node1:7171", "node3:7171"],
        ),
        (
            "examples/cluster/docker/node3.toml",
            "node3:7171",
            ["node1:7171", "node2:7171"],
        ),
    ] {
        let cfg = Config::load(&root.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        cfg.validate_structural()
            .unwrap_or_else(|e| panic!("{file} invalid: {e}"));
        assert_eq!(
            cfg.cluster.advertise_client_addr.as_deref(),
            Some(me),
            "{file} must declare its own client address"
        );
        let declared: Vec<&str> = cfg
            .cluster
            .peers
            .iter()
            .filter_map(|p| p.client_addr.as_deref())
            .collect();
        for peer in peers {
            assert!(
                declared.contains(&peer),
                "{file} must declare peer client_addr {peer}, got {declared:?}"
            );
        }
    }

    // The loopback example configs likewise declare a usable advertise_client_addr
    // (there the hint is host-reachable, so no fallback is needed).
    for (file, me) in [
        ("examples/cluster/node1.toml", "127.0.0.1:7171"),
        ("examples/cluster/node2.toml", "127.0.0.1:7181"),
        ("examples/cluster/node3.toml", "127.0.0.1:7191"),
    ] {
        let cfg = Config::load(&root.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        cfg.validate_structural()
            .unwrap_or_else(|e| panic!("{file} invalid: {e}"));
        assert_eq!(
            cfg.cluster.advertise_client_addr.as_deref(),
            Some(me),
            "{file}"
        );
    }
}

#[test]
fn docker_compose_docs_explain_in_network_vs_host_client_addr() {
    // The operator docs must explain that the Docker Compose leader-client-address
    // hint is the in-Docker-network address (e.g. node2:7171), which is NOT the
    // host-published port (e.g. 127.0.0.1:7181), so a host client re-resolves the
    // leader — the documented fallback, not a failure. This keeps the docs honest
    // and prevents the documented fallback from being read as a bug.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    for (file, needles) in [
        (
            "docs/CLUSTER_TROUBLESHOOTING.md",
            ["in-network", "published port", "re-resolve"],
        ),
        (
            "examples/cluster/README.md",
            ["in-network", "host-published", "re-resolve"],
        ),
    ] {
        let text =
            std::fs::read_to_string(root.join(file)).unwrap_or_else(|e| panic!("read {file}: {e}"));
        for needle in needles {
            assert!(
                text.contains(needle),
                "{file} must explain the in-network vs. host-published client address \
                 (missing {needle:?})"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leader_reports_its_own_client_addr_in_health() {
    // The leader names its OWN client address in health (from
    // advertise_client_addr), closing the gap where a node could not report its
    // own client address — a peer can only ever name another peer's.
    let nodes = start_cluster().await;
    let leader = wait_for_leader(&nodes, Duration::from_secs(15)).await;

    let health = awp_health(&nodes[leader].client_addr).await;
    let cluster = health.cluster.expect("cluster health present");
    assert_eq!(cluster.role, "leader");
    assert_eq!(
        cluster.leader_client_addr.as_deref(),
        Some(nodes[leader].client_addr.as_str()),
        "the leader reports its own client address as the leader client address"
    );

    shutdown_all(&nodes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_status_leader_client_addr_matches_not_leader_hint() {
    // The leader client address a follower reports in cluster health is exactly
    // the address it puts in a `not_leader` hint — one consistent source.
    let nodes = start_cluster().await;
    let leader = wait_for_leader(&nodes, Duration::from_secs(15)).await;
    let follower = (0..nodes.len()).find(|&i| i != leader).unwrap();

    let health = awp_health(&nodes[follower].client_addr).await;
    let from_status = health
        .cluster
        .expect("cluster health present")
        .leader_client_addr;
    assert_eq!(
        from_status.as_deref(),
        Some(nodes[leader].client_addr.as_str())
    );

    let resp = awp_insert(&nodes[follower].client_addr, 3, 2).await;
    let payload: auradb_protocol::ErrorPayload = resp.decode_json().unwrap();
    assert_eq!(payload.code, auradb_core::ErrorCode::NotLeader);
    let from_hint = payload
        .not_leader
        .expect("structured not_leader hints present")
        .leader_client_addr;

    assert_eq!(
        from_status, from_hint,
        "cluster status and the not_leader hint name the same leader client address"
    );

    shutdown_all(&nodes);
}
