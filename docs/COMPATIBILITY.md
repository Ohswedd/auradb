# AuraDB Compatibility Matrix

This document records what AuraDB v0.3.1 implements and how it interoperates with
the Aura Connector client library and the Aura Wire Protocol (AWP). v0.3.1 is a
stabilization release for the MVCC and planner behavior introduced in v0.3.0: it
preserves all v0.3.0 wire behavior, keeps AWP 1 (the health report gains an
additive `mvcc` section and `EXPLAIN ANALYZE` gains additive fields), and
requires no connector release.

| AuraDB | Aura Connector | Protocol | Status |
| ------ | -------------- | -------- | ------ |
| 0.3.1  | 0.3.x          | AWP 1    | Supported (native AuraDB backend) |
| 0.3.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend) |
| 0.2.1  | 0.3.x          | AWP 1    | Supported (native AuraDB backend) |
| 0.2.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend) |
| 0.2.x  | 0.2.x          | n/a      | Not wire compatible (see note below) |
| 0.1.0  | 0.3.x          | AWP 1    | Basic compatible (no auth, no TLS, no document-path/full-text indexes) |

**Note on Aura Connector 0.2.x:** the 0.2.x connector release ships a different
internal framing for its in-process reference backend and cannot complete a
handshake with the AuraDB network server. The coordinated **Aura Connector
0.3.0** adds a native AuraDB-over-TCP backend that speaks AWP 1 (including auth
and TLS). Use Aura Connector 0.3.x to connect to an AuraDB 0.2.x server. See
[AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md).

## Versions

- **AuraDB:** 0.3.1
- **Storage format:** v2 (commit-timestamped MVCC version chains). A v1 (≤ 0.2.x)
  data directory is migrated to v2 transparently on first open; an unknown future
  format is rejected. See [UPGRADING.md](UPGRADING.md).
- **Aura Wire Protocol:** AWP 1 (44-byte framed header, CRC32-checked, JSON
  payloads). See [PROTOCOL.md](PROTOCOL.md).
- **Aura Connector (tested):** 0.3.x

## Required connector features

- AWP 1 framing (`AURA` magic, 44-byte header, 128-bit request id).
- HELLO handshake with optional `auth_token`.
- JSON Query IR (`docs/QUERY_ENGINE.md`).

## Supported query operations

| Operation | Supported | Notes |
| --------- | --------- | ----- |
| find | Yes | filter, order, limit, offset, projection |
| count | Yes | |
| exists | Yes | |
| filter compare (`eq`,`ne`,`lt`,`lte`,`gt`,`gte`,`in`) | Yes | |
| filter `contains` (substring) | Yes | |
| filter `exists` | Yes | |
| filter `contains_text` (full-text) | Yes | uses full-text index or honest scan |
| boolean `and`/`or`/`not` | Yes | |
| document-path filter (dotted) | Yes | uses document-path index when declared |
| order by / limit / offset / projection | Yes | |
| relationship `include` | Yes | to-one and to-many |
| vector nearest | Yes | exact search (`cosine`, `euclidean`, `dot_product`) |
| cursor streaming | Yes | server-side cursors with idle reaping |
| explain | Yes | reports strategy, the index used, and estimated rows/cost |
| explain analyze | Yes | execution metrics via the raw IR `"analyze": true` flag (no new opcode) |
| migration estimate | Yes | |

## Supported schema features

- Fields: `uuid`, `string`, `int`, `float`, `bool`, `timestamp`, `document`,
  `bytes`, `vector { dim }`.
- Field flags: `primary_key`, `unique`, `nullable`, `indexed`.
- Named indexes: `document_path` (dotted path equality), `full_text` (tokenized
  inverted index on a string field).
- Relationships: to-one and to-many links with `restrict` / `set_null` on delete.

## Supported vector operations

- Exact nearest-neighbour search over a fixed-dimension vector field.
- Metrics: `cosine`, `euclidean`, `dot_product`.
- Not supported: approximate nearest neighbour (ANN), HNSW.

## Supported document operations

- Nested document fields and dotted-path access in filters and ordering.
- Document-path equality indexes.

## Supported relationship operations

- `include` hydration of linked records (to-one and to-many).
- Referential integrity on delete (`restrict`) and dangling-link reads
  (`set_null`).

## Authentication

- Optional static-token authentication, enforced when enabled.
- Tokens are verified against an Argon2id hash; never stored or compared in
  plaintext. See [SECURITY.md](SECURITY.md).

## TLS

- Optional server-terminated TLS (rustls). Mutual TLS (client certificates) is
  supported. Fail-closed validation: missing or invalid material aborts startup.

## Known limitations

- Single node only. No clustering, replication, sharding, or Raft.
- AuraDB v0.3.0 implements single-node snapshot isolation with optimistic write
  conflict detection. It is not serializable isolation (it does not prevent
  write-skew).
- Vector search is exact; there is no ANN/HNSW index.
- Full-text search is tokenized boolean-AND matching with term-frequency
  ranking; it is not BM25.
- Aura Connector 0.2.x cannot connect; use 0.3.x.

## Verification

- **Test date:** 2026-06-05
- **CI workflows:** `ci.yml` (build, fmt, clippy, test, benchmark compilation),
  `conformance.yml` (Python AWP conformance: auth disabled, auth enabled, and
  TLS, plus the Aura Connector smoke and conformance suites), `security.yml`
  (cargo audit and deny), `docker.yml`, `release.yml`.
- **Conformance harness:** `tests/conformance/python/run_conformance.py`.
- **Connector smoke:** `tests/conformance/python/run_connector_smoke.py`.
