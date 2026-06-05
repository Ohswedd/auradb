# Aura Connector Compatibility

This document is the connector-focused companion to the
[Compatibility Matrix](COMPATIBILITY.md). It records which Aura Connector release
talks to AuraDB 0.3.0, what it can drive, and what it cannot. AuraDB 0.3.0 adds
MVCC and a cost-based query planner; it preserves the v0.2.1 wire behavior, so the
same connector compatibility applies and **no connector release is required**.

## Summary

- **Aura Connector 0.3.x remains fully compatible with AuraDB 0.3.0. No connector
  release is required.** The MVCC and query-planner changes are server-side and
  ride the existing AWP 1 wire format and Query IR.
- **AuraDB 0.3.0 speaks AWP 1** (the 44-byte framed Aura Wire Protocol header,
  CRC32-checked, with JSON payloads). See [PROTOCOL.md](PROTOCOL.md).
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
| 0.3.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend) |
| 0.2.1  | 0.3.x          | AWP 1    | Supported (native AuraDB backend) |
| 0.2.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend) |
| 0.2.x  | 0.2.x          | n/a      | Not wire compatible |

The connector side is exercised by `run_connector_smoke.py` (a minimal real
scenario) and `run_connector_conformance.py` (the full suite) in
`conformance.yml`, against servers with auth disabled and with auth plus TLS.

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

- Clustering, replication, sharding, and Raft (AuraDB is single node).
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
