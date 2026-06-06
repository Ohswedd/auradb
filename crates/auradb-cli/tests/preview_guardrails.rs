//! v0.6.0 preview guardrail tests.
//!
//! The multi-node mode is an experimental, opt-in **preview** — not production
//! HA. These tests pin the guardrails that keep that true: it stays opt-in,
//! static-membership only, TLS+token gated when public, and the docs disclaim
//! production HA rather than claim it.

use std::process::Command;

use auradb_cluster::{ClusterConfig, ClusterTlsConfig, PeerConfig, Secret};

fn peer(node_id: &str, addr: &str) -> PeerConfig {
    PeerConfig {
        node_id: node_id.into(),
        addr: addr.into(),
        client_addr: None,
    }
}

#[test]
fn multi_node_still_requires_preview_flag() {
    let mut cfg = ClusterConfig::single_node();
    cfg.peers = vec![peer("00000000000000a2", "127.0.0.1:7272")];
    // A peer set without the explicit experimental opt-in fails closed.
    let err = cfg.validate().unwrap_err();
    assert!(
        err.to_string().contains("experimental_multi_node"),
        "peers without experimental_multi_node must be rejected: {err}"
    );
    // With the opt-in, a loopback preview cluster validates.
    cfg.experimental_multi_node = true;
    assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    assert!(cfg.is_multi_node());
}

#[test]
fn public_cluster_requires_tls_and_peer_auth() {
    let mut cfg = ClusterConfig::single_node();
    cfg.experimental_multi_node = true;
    cfg.allow_experimental_public_cluster = true;
    cfg.listen_addr = "10.0.0.1:7272".into();
    cfg.advertise_addr = "10.0.0.1:7272".into();
    cfg.peers = vec![peer("00000000000000a2", "10.0.0.2:7272")];
    // Non-loopback peer traffic without TLS is refused.
    assert!(cfg.validate().unwrap_err().to_string().contains("peer TLS"));
    // TLS without a shared token is refused.
    cfg.tls = ClusterTlsConfig {
        enabled: true,
        cert_path: Some("cert.pem".into()),
        key_path: Some("key.pem".into()),
        ca_path: Some("ca.pem".into()),
    };
    assert!(cfg
        .validate()
        .unwrap_err()
        .to_string()
        .contains("peer authentication token"));
    // TLS plus a token is accepted.
    cfg.peer_auth_token = Secret::new("a-shared-secret");
    assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
}

#[test]
fn dynamic_membership_commands_absent() {
    // The preview is static-membership only: there is no join/leave/add/remove
    // cluster subcommand. Assert the CLI surface offers none.
    let exe = env!("CARGO_BIN_EXE_auradb");
    let out = Command::new(exe)
        .args(["cluster", "--help"])
        .output()
        .expect("run `auradb cluster --help`");
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_lowercase();
    for forbidden in [
        "join",
        "leave",
        "add-node",
        "remove-node",
        "add-peer",
        "remove-peer",
    ] {
        assert!(
            !help.contains(forbidden),
            "cluster help unexpectedly offers a dynamic-membership command `{forbidden}`:\n{help}"
        );
    }
}

#[test]
fn preview_docs_do_not_claim_production_ha() {
    // The preview docs must explicitly disclaim production HA rather than claim
    // it. Assert each key doc carries the honest disclaimer.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for doc in [
        "docs/V0_6_RELEASE_NOTES.md",
        "docs/CLUSTERING.md",
        "README.md",
    ] {
        let text = std::fs::read_to_string(root.join(doc))
            .unwrap_or_else(|e| panic!("read {doc}: {e}"))
            .to_lowercase()
            // Drop markdown emphasis so "_not_ production HA" still matches.
            .replace(['_', '*'], "");
        assert!(
            text.contains("not production ha"),
            "{doc} must explicitly disclaim production HA (\"not production HA\")"
        );
        // And never the bare marketing claims.
        for bad in ["production-ready cluster", "production-grade ha"] {
            assert!(!text.contains(bad), "{doc} must not claim '{bad}'");
        }
    }
}
