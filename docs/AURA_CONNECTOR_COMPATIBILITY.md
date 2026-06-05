# Aura Connector Compatibility

This document is the connector-focused companion to the
[Compatibility Matrix](COMPATIBILITY.md). It records which Aura Connector release
talks to AuraDB 0.5.1, what it can drive, and what it cannot.

> **AuraDB v0.5.1 hardens the controlled multi-node preview. Single-node mode
> remains the recommended production mode.**

AuraDB 0.5.1 hardens the experimental cross-process multi-node preview, but it
preserves the existing wire behavior, so the same connector compatibility applies
and **no connector release is required**. The wire additions remain additive: the
health report's `cluster` section gains additional diagnostics fields
(`preview_multi_node`, `quorum_available`, a `peers` array, and per-peer
reachability detail), the error payload gains an optional `retryable` hint, and
the `not_leader` error code is unchanged — a 0.3.x connector handles all of these
safely (it ignores unknown fields and maps unknown error codes to a generic
server error). A connector targets the **leader's client address**; a write
routed to a follower returns `not_leader` with a leader hint embedded in the
human-readable message.

## Summary

- **Aura Connector 0.3.x remains fully compatible with AuraDB 0.5.1. No connector
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
