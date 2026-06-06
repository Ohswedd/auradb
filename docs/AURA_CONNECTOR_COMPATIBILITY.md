# Aura Connector Compatibility

This document is the connector-focused companion to the
[Compatibility Matrix](COMPATIBILITY.md). It records which Aura Connector release
talks to AuraDB 0.7.1, what it can drive, and what it cannot.

> **AuraDB v0.7.x adds connector cluster ergonomics for the controlled multi-node
> preview. It is _not_ production HA — there is no automatic failover,
> linearizable follower reads, or distributed transactions. Single-node mode
> remains the recommended production mode.**

## Connector ergonomics polish (v0.7.1)

v0.7.1 is a **coordinated patch** release with **Aura Connector v0.4.1**, a
docs/ergonomics polish over v0.4.0. The server is unchanged — the `not_leader`
payload below is byte-for-byte the same as v0.7.0, and the Aura Wire Protocol (AWP
1) is unchanged. Aura Connector v0.4.1 improves the client-side experience around
this payload without changing the wire contract:

- clearer `AuraNotLeaderError` messages (the rendered string names the node
  reached, the leader address or that it is unknown, the retry classification, and
  the redirect call — and still leaks no secrets);
- a secure-by-default redirect: an explicit insecure redirect target is refused
  unless the caller opts in, and TLS/auth are preserved across a reconnect;
- transaction-redirect safety documentation and tests.

Aura Connector 0.4.0 and 0.3.x remain compatible with AuraDB 0.7.1; the v0.4.1
improvements are client-side only.

## Connector cluster ergonomics (v0.7.0)

v0.7.0 is the first **coordinated** server + connector cluster-ergonomics release:
**Aura Connector v0.4.0** ships alongside it. The server-side change is additive
and backward compatible — the `not_leader` error frame now carries a structured
`not_leader` object built from the node's current cluster view:

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

Every field is present only when genuinely known (a follower that has not yet
recognized a leader omits the leader fields rather than guessing), the object
carries no secrets, and older clients ignore it — so **Aura Connector 0.3.x stays
fully compatible**. The structured-payload contract is pinned by
`crates/auradb-server/tests/cluster_preview.rs`
(`not_leader_payload_includes_leader_client_addr_when_known`,
`not_leader_payload_contains_no_secrets`) and
`crates/auradb-server/tests/not_leader.rs`
(`not_leader_payload_safe_over_tls_auth`), with the wire shape covered by
`auradb-protocol` unit tests.

**Aura Connector 0.4.x** consumes these fields:

- maps `not_leader` to a dedicated `AuraNotLeaderError` exposing `leader_addr`,
  `leader_client_addr`, `leader_hint`, `leader_node_id`, `current_node_id`,
  `retryable`, and the full `raw_payload`;
- `Client.connect_to_leader(error)` / `Client.reconnect_to(addr)` open a new
  client bound to the leader, preserving the original auth and TLS settings;
- `Client.with_leader_redirect(max_redirects=…)` is an opt-in, bounded redirect
  for autonomous writes (never for transactions or streaming cursors).

The connector cluster conformance runner
`tests/conformance/python/run_connector_cluster.py` exercises this end to end
against a live preview cluster (leader write, follower `not_leader`, reconnect,
bounded redirect, transaction-not-redirected), gated on
`AURADB_CLUSTER_LEADER_DSN` / `AURADB_CLUSTER_FOLLOWER_DSN`.

Manual leader routing (unchanged, still available) resolves the leader with
`auradb cluster leader --addr <any-node>` and points the connector there.

## Connector leader-hint review (v0.6.2)

For v0.6.2 we again reviewed whether the connector needs a patch and again chose
**docs-only (Option A)**. The v0.6.2 changes are server-side recovery hardening
plus one additive `leader_changes` field on the cluster health report; nothing in
the wire contract or the `not_leader` behavior changed, so **no connector release
is required** and Aura Connector 0.3.x stays fully compatible. The leader-hint
behavior reviewed below is unchanged: the `not_leader` contract is still pinned by
`crates/auradb-server/tests/not_leader.rs` (the connector stays safe on a
not-leader write and never retries forever), and the server-side recovery a
connector rides on after a leadership change is covered by the
repeated-leader-restart tests in `crates/auradb-replication/tests/multi_node.rs`.
The Python connector conformance smoke
(`tests/conformance/python/run_connector_smoke.py`) continues to run against a
live leader in CI.

Manual leader routing, end to end:

```bash
# 1. Resolve the current leader's client address from any node.
auradb cluster leader --addr 127.0.0.1:7101 --json
# -> { "leader_id": "...", "leader_client_addr": "127.0.0.1:7001", ... }

# 2. Point the connector at that client address and issue writes there.
#    (In Python with Aura Connector 0.3.x:)
#      client = AuraClient("127.0.0.1:7001")
#
# 3. If leadership has moved, a write to the old leader raises an
#    AuraServerError whose message contains "not_leader" and names the new
#    leader. Re-resolve with `auradb cluster leader` and reconnect — the server
#    returns exactly one terminal not_leader and never retries internally.
```

## Connector leader-hint UX review (v0.6.1)

For v0.6.1 we reviewed whether the connector needs a patch and chose **docs-only
(Option A)**: Aura Connector 0.3.x stays fully compatible but is **not
cluster-routing-aware**, and the v0.6.1 wire changes are additive, so no
connector release is required.

In a multi-node preview cluster, route writes to the leader **manually**:

- Point the connector at the leader's client address. Resolve it with
  `auradb cluster leader --addr <any-node>` (or read the `leader_client_addr`
  field from `auradb cluster status --addr <any-node> --json`).
- A write sent to a follower returns a structured `not_leader` error (stable code
  `not_leader`, marked `retryable`) whose human-readable message names the
  current leader and, when the operator declared a peer `client_addr`, its client
  address. A 0.3.x connector surfaces this as a retryable server error; reconnect
  to the leader's address and retry the write.
- The server returns exactly one terminal `not_leader` per write and never loops
  internally, so a client is never left hanging. The leader-hint message and the
  no-infinite-retry contract are pinned by
  `crates/auradb-server/tests/not_leader.rs`
  (`connector_not_leader_message_includes_leader_hint`, `connector_no_infinite_retry`).

A future connector could expose a dedicated `AuraNotLeaderError` with parsed
`leader_hint` / `leader_addr` and an opt-in manual-reconnect helper, but that is
not part of v0.6.1 and would be a coordinated connector release. The preview does
**not** do automatic write redirection or transaction redirect.

AuraDB 0.6.0 improves the experimental cross-process multi-node preview
(fail-stop recovery validation, peer snapshot install, sharper diagnostics), but
it preserves the existing wire behavior, so the same connector compatibility
applies and **no connector release is required**. After a leader kill, a write to
a non-leader returns the structured `not_leader` error with a leader hint and a
retryable flag; Aura Connector 0.3.x surfaces this as a retryable server error
and a client retry against the new leader succeeds. The wire additions remain
additive: the
health report's `cluster` section gains additional diagnostics fields
(`preview_multi_node`, `quorum_available`, a `peers` array, and per-peer
reachability detail), the error payload gains an optional `retryable` hint, and
the `not_leader` error code is unchanged — a 0.3.x connector handles all of these
safely (it ignores unknown fields and maps unknown error codes to a generic
server error). A connector targets the **leader's client address**; a write
routed to a follower returns `not_leader` with a leader hint embedded in the
human-readable message.

## Summary

- **Aura Connector 0.3.x remains fully compatible with AuraDB 0.6.2. No connector
  release is required.** Cluster mode and the multi-node preview are server-side
  and ride the existing AWP 1 wire format and Query IR; the `cluster` health
  section (including the additive per-peer diagnostics fields), the optional
  additive `retryable` error hint, and the `not_leader` error code are additive
  and optional.
- **A connector connects to the leader.** In a multi-node cluster, point the
  connector at the leader's client address (use `auradb cluster leader` or the
  `cluster` status section); a write sent to a follower returns `not_leader`.
- **AuraDB 0.5.0 speaks AWP 1** (the 44-byte framed Aura Wire Protocol header,
  CRC32-checked, with JSON payloads), unchanged from prior releases. See
  [PROTOCOL.md](PROTOCOL.md).
- **Use Aura Connector 0.3.x.** The published Aura Connector 0.3.x ships a native
  AuraDB-over-TCP backend that speaks AWP 1, including authentication and TLS.
- **`EXPLAIN ANALYZE` is reachable today through the raw Query IR.** It is
  requested as an optional `"analyze": true` sibling key in the raw Query IR sent
  to the existing `Explain` opcode — there is no new opcode and no protocol break,
  so an existing 0.3.x connector reaches it via raw IR without any update.
- **Aura Connector 0.2.x is not wire compatible.** The 0.2.x connector uses a
  different internal framing for its in-process reference backend and cannot
  complete an AWP handshake with the AuraDB network server. Upgrade to 0.3.x.

| AuraDB | Aura Connector | Protocol | Status |
| ------ | -------------- | -------- | ------ |
| 0.7.1  | 0.4.1          | AWP 1    | Supported, recommended (clearer `AuraNotLeaderError` messages, secure-by-default redirect, transaction-redirect docs; identical wire payload to 0.7.0) |
| 0.7.1  | 0.4.0          | AWP 1    | Supported (structured `not_leader` payload; `AuraNotLeaderError`, reconnect + bounded redirect helpers) |
| 0.7.1  | 0.3.x          | AWP 1    | Supported (compatible; ignores the additive `not_leader` object and routes the leader manually) |
| 0.7.0  | 0.4.x          | AWP 1    | Supported (native AuraDB backend; structured `not_leader` payload; `AuraNotLeaderError`, reconnect + bounded redirect helpers) |
| 0.7.0  | 0.3.x          | AWP 1    | Supported (compatible; ignores the additive `not_leader` object and routes the leader manually) |
| 0.6.2  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; additive `leader_changes` diagnostics field; manual leader routing) |
| 0.6.1  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; additive snapshot/lag diagnostics fields; manual leader routing) |
| 0.6.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; additive fail-stop diagnostics fields; targets the leader) |
| 0.5.1  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; additive diagnostics fields; targets the leader) |
| 0.5.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; cluster fields additive; targets the leader) |
| 0.4.1  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; cluster fields additive) |
| 0.4.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; cluster fields additive) |
| 0.3.1  | 0.3.x          | AWP 1    | Supported (native AuraDB backend) |
| 0.3.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend) |
| 0.2.1  | 0.3.x          | AWP 1    | Supported (native AuraDB backend) |
| 0.2.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend) |
| 0.2.x  | 0.2.x          | n/a      | Not wire compatible |

The connector side is exercised by `run_connector_smoke.py` (a minimal real
scenario) and `run_connector_conformance.py` (the full suite) in
`conformance.yml`, against servers with auth disabled and with auth plus TLS.

For v0.5.0, the published `aura-connector` 0.3.0 smoke suite was run against
the **elected leader** of a three-node loopback preview cluster (12/12 checks
passed, including the additive cluster health fields). v0.5.1 preserves this
behavior and adds `not_leader` ergonomics tests (the leader hint and retryable
guidance, and that the same connection stays usable after a `not_leader`
response); the published-connector smoke against the elected leader continues to
be exercised by the conformance and cluster CI workflows. The full
`run_connector_conformance.py` suite and the auth/TLS connector matrix continue
to run in `conformance.yml`.

For **v0.6.0**, the published `aura-connector` 0.3.0 was installed from PyPI and
run locally against a v0.6.0 server: the AWP protocol conformance passed 18/18,
the connector smoke 12/12, and the full connector conformance 15/15. No connector
changes are required; AWP stays at v1, and the additive v0.6.0 fail-stop
diagnostics fields on the health report are ignored by the 0.3.x connector.

For **v0.6.1**, the connector contract is unchanged. Aura Connector 0.3.x remains
wire-compatible: the additive v0.6.1 snapshot/lag diagnostics fields on the health
report and per-peer status are ignored by the 0.3.x connector. Local validation
used the stdlib AWP harness (`run_conformance.py`, 18/18) and the Rust conformance
crate (`auradb-conformance`); published Aura Connector conformance is covered by
CI (`conformance.yml`) and must pass before release. The v0.6.1 leader-hint review
(above) kept the connector as-is (Option A, docs-only).

For **v0.6.2**, the connector contract is again unchanged. The release is
server-side recovery hardening (repeated leader restart, larger-state recovery,
multi-model snapshot install, reconnect storms, partition/heal); the only wire
addition is the additive `leader_changes` field on the cluster health report,
which the 0.3.x connector ignores. The v0.6.2 leader-hint review kept the
connector as-is (Option A, docs-only).

## Required connector extras

- AWP 1 framing (`AURA` magic, 44-byte header, 128-bit request id).
- HELLO handshake support, including the optional `auth_token` field for the
  authentication fast path, and reading `auth_required` / `authenticated` from
  the HELLO_ACK.
- The AUTH / AUTH_RESULT opcodes and the `unauthenticated` /
  `invalid_credentials` error codes (for the dedicated AUTH-frame path).
- TLS client support that trusts the server CA (and presents a client
  certificate when the server requires mutual TLS).
- The JSON Query IR documented in [QUERY_ENGINE.md](QUERY_ENGINE.md).

## Tested scenarios

The connector and the conformance harness drive the same server over the wire.
Tested scenarios:

- ping, health
- schema create, insert, find, filter, document field
- document-path index lookup (with an EXPLAIN check)
- full-text search (with an EXPLAIN check)
- relationship include (to-one and to-many)
- vector nearest (exact)
- explain, count, exists, migration estimate
- update, upsert, delete
- transaction commit and rollback
- transaction-scoped reads (read-your-writes within a transaction; staged writes
  invisible to non-transactional readers until commit)

The pure-standard-library Python conformance client at
`tests/conformance/python/run_conformance.py` runs these scenarios and accepts
`--auth-token`, `--tls-ca`, and `--tls-server-name`. See
[CONFORMANCE.md](CONFORMANCE.md).

## `not_leader` handling (v0.4.1)

A write is only accepted by the leader. On a non-leader node the server returns a
structured error frame with the additive `not_leader` error code and a
human-readable leader hint. This is validated over the wire in v0.4.1
(`crates/auradb-server/tests/not_leader.rs`):

- the write comes back as a single, prompt `not_leader` error — the server never
  retries internally, so a client receives a terminal response rather than a hang;
- the connection stays healthy afterward (a subsequent request gets a normal
  response, with auth/TLS state intact).

A 0.3.x connector that does not model `not_leader` specifically maps the unknown
code to its generic server-error type and surfaces it to the caller — it does not
crash, does not retry forever, and does not drop auth/TLS state. This was checked
directly against the published `aura-connector` 0.3.0: the `not_leader` code falls
back to `AuraServerError`, arrives with `retryable = False` (the wire frame omits
the field), and the connector's retry policy is bounded (`max_attempts = 3`). No
connector release is required.

In single-node cluster mode the sole node is always the leader, so `not_leader`
does not arise in normal operation. In the **v0.5.0 multi-node preview**, point
the connector at the **leader's client address** (from `auradb cluster leader` or
the `cluster` status section); a write routed to a follower returns `not_leader`
with a leader hint and the connection stays healthy. `not_leader` handling is
additive — no connector change is required for the preview.

## Supported features

- Authentication: enforced static-token auth (Argon2id-verified) when the server
  enables it.
- TLS: server-terminated TLS and optional mutual TLS.
- Query: find/filter/order/limit/offset/projection, `contains`, `contains_text`,
  `exists`, boolean `and`/`or`/`not`, document-path equality, count, exists,
  relationship includes, exact vector nearest, EXPLAIN, `EXPLAIN ANALYZE` (via the
  raw IR `"analyze": true` flag), migration estimate.
- Mutations: insert, bulk insert, update, delete, upsert.
- Transactions: begin/commit/rollback with snapshot reads pinned at `begin` and
  read-your-writes. Reads carrying a transaction id observe committed state as of
  the transaction's snapshot overlaid with its staged writes and deletes, across
  find, filter, count, exists, explain, vector, document-path, full-text,
  relationship include, and cursor paging. AuraDB v0.3.0 implements single-node
  snapshot isolation with optimistic write conflict detection. It is not
  serializable isolation.
- Server-side cursors with idle reaping.

## Known unsupported features

- Production multi-node clustering, automatic failover, and sharding. v0.5.0 adds
  a controlled, experimental multi-node preview (off by default, gated by two
  opt-ins), but the recommended production path remains single-node. There is
  nothing the connector must do for the preview beyond targeting the leader — the
  `not_leader` error and the additive `cluster` health fields are handled safely
  by a 0.3.x connector. There are no distributed transactions, linearizable reads,
  or follower reads (followers reject reads), and no dynamic membership.
- Serializable isolation (AuraDB implements single-node snapshot isolation with
  optimistic write conflict detection, not serializable isolation).
- Approximate nearest neighbour (ANN/HNSW); vector search is exact.
- BM25 and hybrid fusion ranking; full-text is tokenized boolean-AND matching
  with term-frequency ranking. See [FULL_TEXT.md](FULL_TEXT.md).
- RBAC, field-level encryption, encryption at rest, and audit logging.

## See also

- [COMPATIBILITY.md](COMPATIBILITY.md) for the full capability matrix.
- [SECURITY.md](SECURITY.md) for the auth and TLS model.
- [PROTOCOL.md](PROTOCOL.md) for the wire format.
