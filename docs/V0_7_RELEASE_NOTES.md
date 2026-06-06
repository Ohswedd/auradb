# AuraDB v0.7.0 release notes

**Theme: Connector cluster ergonomics.**

AuraDB v0.7.0 gives Python clients a clean, safe, cluster-aware experience for the
controlled multi-node **preview**, coordinated with **Aura Connector v0.4.0**. It
is **not production HA. Single-node mode remains the recommended production
mode.**

This release makes no production automatic-failover claim, and it does not
implement linearizable follower reads, distributed transactions, dynamic
membership, sharding, or multi-region — and it claims none of them. Multi-node
mode stays experimental, off by default, and gated behind two explicit opt-ins.

## What changed

### Stable, structured `not_leader` payload

A write reaches the cluster leader only; a write sent to a follower is rejected
with a `not_leader` response. v0.7.0 enriches that response with an additive,
machine-readable `not_leader` object built from the node's current cluster view:

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

- Fields are present only when genuinely known — a follower that has not yet
  recognized a leader omits the leader fields rather than guessing.
- The object carries no secrets and works identically over plaintext and
  TLS/auth connections.
- It is purely additive: the Aura Wire Protocol stays at **AWP 1**, and older
  clients (including Aura Connector 0.3.x) ignore the object and continue to route
  the leader manually using the human message or `auradb cluster leader`.

### Leader discovery (unchanged, already available)

`auradb cluster leader --addr <node>` reports the current leader and its client
address (`leader_client_addr`), the basis for resolving the leader before a write.

### Coordinated connector ergonomics (Aura Connector v0.4.0)

Aura Connector v0.4.0 consumes the structured payload: it maps `not_leader` to a
dedicated `AuraNotLeaderError`, and adds `Client.connect_to_leader(error)` /
`Client.reconnect_to(addr)` (auth and TLS preserved) plus an opt-in, bounded
`Client.with_leader_redirect(...)`. Transactions and streaming cursors are never
auto-redirected.

## Validation

- `auradb-protocol` unit tests pin the payload shape, the omit-unknown-fields
  behavior, and the no-secrets guarantee.
- `crates/auradb-server/tests/cluster_preview.rs` verifies the populated payload
  over a live three-node cluster
  (`not_leader_payload_includes_leader_client_addr_when_known`,
  `not_leader_payload_contains_no_secrets`).
- `crates/auradb-server/tests/not_leader.rs` verifies the payload over an
  authenticated session (`not_leader_payload_safe_over_tls_auth`) and the
  no-infinite-retry / connection-stays-healthy contract.
- `tests/conformance/python/run_connector_cluster.py` drives Aura Connector
  v0.4.x against a live preview cluster end to end.

## Compatibility

- Aura Connector 0.4.x ↔ AuraDB 0.7.x over AWP 1, with the new cluster ergonomics.
- Aura Connector 0.3.x remains fully compatible (ignores the additive object).
- The on-disk storage format is unchanged. See [COMPATIBILITY.md](COMPATIBILITY.md)
  and [AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md).
