//! Regression test: certificates produced by `auradb cert generate-dev` must work
//! for the multi-node peer (cluster) transport, which uses **mutual TLS**.
//!
//! The peer transport has each node present its certificate as a *client*
//! certificate when it dials a peer, so a server-only certificate is rejected by
//! the peer's client-cert verifier. This forms a real two-node cluster over
//! loopback using the actual `cmd_cert_generate_dev` output and asserts a leader
//! is elected — which only happens if the generated certificates allow both
//! server and client authentication.

use std::net::TcpListener as StdTcpListener;
use std::time::{Duration, Instant};

use auradb::core::{CollectionSchema, FieldDef, FieldType};
use auradb::Engine;
use auradb_cluster::{
    ClusterConfig, ClusterId, ClusterIdentity, ClusterMetadata, ClusterTlsConfig, NodeId,
    NodeMetadata, NodeRole, PeerConfig, Secret, METADATA_FORMAT_VERSION,
};
use auradb_replication::PeerCluster;

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

fn identity(cluster_id: ClusterId, node_id: NodeId) -> ClusterIdentity {
    ClusterIdentity {
        node: NodeMetadata {
            format_version: METADATA_FORMAT_VERSION,
            node_id,
            created_by_version: "0.5.1-test".into(),
        },
        cluster: ClusterMetadata {
            format_version: METADATA_FORMAT_VERSION,
            cluster_id,
            created_by_version: "0.5.1-test".into(),
        },
    }
}

fn schema() -> CollectionSchema {
    CollectionSchema::new("C").with_field(FieldDef {
        name: "id".into(),
        field_type: FieldType::Int,
        primary_key: true,
        unique: true,
        nullable: false,
        indexed: false,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generated_dev_certs_work_for_peer_mutual_tls() {
    let dir = tempfile::tempdir().unwrap();
    let certs = dir.path().join("certs");
    // Generate per-node certs with the REAL CLI cert tooling. The peers dial each
    // other at 127.0.0.1, so the server name verified is "127.0.0.1" — the cert
    // must carry that SAN.
    for node in ["node1", "node2"] {
        auradb_cli::cmd_cert_generate_dev(
            &certs,
            Some(node.to_string()),
            vec!["127.0.0.1".to_string()],
        )
        .unwrap();
    }
    let ca = certs.join("ca.crt");

    let cluster_id = ClusterId::new(0xC0FFEE).unwrap();
    let ports = reserve_ports(2);
    let ids = [NodeId::from_raw(1), NodeId::from_raw(2)];
    let addrs: Vec<String> = ports.iter().map(|p| format!("127.0.0.1:{p}")).collect();

    let tls_for = |node: &str| ClusterTlsConfig {
        enabled: true,
        cert_path: Some(certs.join(format!("{node}.crt"))),
        key_path: Some(certs.join(format!("{node}.key"))),
        ca_path: Some(ca.clone()),
    };

    let mut nodes = Vec::new();
    let mut engines = Vec::new();
    for i in 0..2 {
        let engine = Engine::open(dir.path().join(format!("data{i}"))).unwrap();
        engine.create_schema(schema()).unwrap();
        let j = 1 - i;
        let cfg = ClusterConfig {
            enabled: true,
            experimental_multi_node: true,
            allow_experimental_public_cluster: false,
            cluster_id: cluster_id.to_string(),
            node_id: ids[i].to_string(),
            listen_addr: addrs[i].clone(),
            advertise_addr: addrs[i].clone(),
            bootstrap: true,
            peers: vec![PeerConfig {
                node_id: ids[j].to_string(),
                addr: addrs[j].clone(),
                client_addr: None,
            }],
            peer_auth_token: Secret::new("shared-token"),
            tls: tls_for(if i == 0 { "node1" } else { "node2" }),
        };
        let id = identity(cluster_id, ids[i]);
        let peer = PeerCluster::spawn(
            engine.clone(),
            id,
            cfg,
            dir.path().join(format!("cluster{i}")),
        )
        .unwrap();
        engines.push(engine);
        nodes.push(peer);
    }

    // A leader is only elected if the two nodes complete the mutual-TLS peer
    // handshake — i.e. each generated certificate is accepted as a client cert.
    let deadline = Instant::now() + Duration::from_secs(15);
    let elected = loop {
        let leaders = nodes
            .iter()
            .filter(|n| n.status().role == NodeRole::Leader)
            .count();
        let connected = nodes.iter().any(|n| n.quorum_available());
        if leaders == 1 && connected {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    for n in &nodes {
        n.shutdown().await;
    }
    assert!(
        elected,
        "generated dev certs must allow the mutual-TLS peer transport to form a cluster \
         (a server-only EKU would be rejected as a client certificate)"
    );
}
