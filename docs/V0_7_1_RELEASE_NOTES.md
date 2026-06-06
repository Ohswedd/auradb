# AuraDB v0.7.1 release notes

**Connector ergonomics polish.** AuraDB v0.7.1 is a coordinated **patch** release
with **Aura Connector v0.4.1**. It improves the developer experience around the
controlled multi-node **preview** for Python connector users: clearer
compatibility documentation, hardened connector cluster conformance guidance, and
additional leader-hint and safe-redirect examples.

This release adds **no** new database architecture. It changes **no** Raft,
storage, query, MVCC, replication, or snapshot semantics. The `not_leader` payload
is **byte-for-byte identical** to v0.7.0, the on-disk storage format is unchanged,
and the Aura Wire Protocol (AWP v1) is unchanged. All v0.7.0 behavior is preserved.

It is **not** production HA. There is no production automatic failover, no
linearizable follower reads, no distributed transactions, no dynamic membership,
and no sharding or multi-region. Multi-node mode remains an experimental, opt-in
preview gated behind `enabled = true` and `experimental_multi_node = true`;
**single-node mode remains the recommended production mode.**

## What's in it

### Coordinated with Aura Connector v0.4.1

Aura Connector v0.4.1 is a client-side ergonomics polish over v0.4.0. Against an
unchanged AuraDB `not_leader` payload it adds:

- clearer `AuraNotLeaderError` messages — the rendered string names the non-leader
  node reached, the leader address (or that it is unknown), the retry
  classification, and the redirect call, and still leaks no secrets;
- a secure-by-default redirect — an explicit insecure redirect target is refused
  unless the caller opts in, and TLS/auth are preserved across a reconnect;
- transaction-redirect safety documentation and tests.

See the connector's `docs/V0_4_1_RELEASE_NOTES.md`.

### Documentation

- `docs/AURA_CONNECTOR_COMPATIBILITY.md`, `docs/COMPATIBILITY.md`, and
  `docs/CONFORMANCE.md` now record v0.7.1 ↔ Aura Connector v0.4.1, with the older
  rows preserved.
- `docs/CLUSTERING.md` and `README.md` carry a v0.7.1 preview note.
- `docs/RELEASE.md` documents the connector-first coordinated release flow.

### Conformance

- The `cluster.yml` loopback job installs the published connector in the
  `>=0.4.1,<0.5` line and runs three scenarios when it is available: connector
  smoke against the leader, connector conformance against the leader, and cluster
  ergonomics across leader + follower.
- On PR/push the step **skips with a clear message** when the published connector
  is not yet installable; for release/tag conformance, run the workflow with the
  `require_published_connector` input so a missing/too-old connector **fails**
  instead of skipping. We do not claim a connector version is published before it
  actually is.

### Examples

- `examples/cluster/python_connector.py` and `examples/cluster/README.md` point at
  leader-hint discovery via `auradb cluster leader` and the connector's
  reconnect/redirect helpers for topologies where the leader address is reachable.

## Payload compatibility (unchanged)

The additive structured `not_leader` object is the same as v0.7.0:

```json
{
  "code": "not_leader",
  "message": "this node (0000000000000001) is not the leader; current leader is node 00000000000000aa (client address 127.0.0.1:7373); retry the write against the leader",
  "retryable": true,
  "not_leader": {
    "current_node_id": "0000000000000001",
    "leader_node_id": "00000000000000aa",
    "leader_client_addr": "127.0.0.1:7373",
    "leader_hint": "127.0.0.1:7373",
    "term": 7,
    "role": "follower"
  }
}
```

Its contract is pinned by `crates/auradb-server/tests/cluster_preview.rs`
(`not_leader_payload_includes_leader_client_addr_when_known`,
`not_leader_payload_contains_no_secrets`) and
`crates/auradb-server/tests/not_leader.rs` (`not_leader_payload_safe_over_tls_auth`).

## Upgrading

No action required. v0.7.1 is a drop-in replacement for v0.7.0; data directories,
configuration, the wire protocol, and the `not_leader` payload are unchanged.
