# AuraDB Compatibility Matrix

This document records what AuraDB v0.6.0 implements and how it interoperates with
the Aura Connector client library and the Aura Wire Protocol (AWP). v0.6.0
improves the controlled multi-node preview and validates fail-stop recovery
(off by default); single-node mode remains the recommended production mode. It
keeps **AWP 1** unchanged and makes no incompatible protocol change: the health
report's `cluster` section gains additional additive fail-stop diagnostics
fields and the error payload's optional `retryable` hint are both ignored by
older clients, so no connector release is required and Aura Connector 0.3.x
remains fully compatible. The on-disk **storage format is unchanged** from
v0.4.x.

| AuraDB | Aura Connector | Protocol | Status |
| ------ | -------------- | -------- | ------ |
| 0.6.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; additive fail-stop diagnostics fields; multi-node preview ergonomics) |
| 0.5.2  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; additive diagnostics fields; multi-node preview cert fix) |
| 0.5.1  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; additive diagnostics fields; multi-node preview hardening) |
| 0.5.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; cluster fields additive; multi-node preview) |
| 0.4.1  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; cluster fields additive) |
| 0.4.0  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; cluster fields additive) |
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

- **AuraDB:** 0.6.0
- **Storage format:** v2 (commit-timestamped MVCC version chains), unchanged from
  v0.4.x. A v1 (≤ 0.2.x) data directory is migrated to v2 transparently on first
  open; an unknown future format is rejected. See [UPGRADING.md](UPGRADING.md).
- **Aura Wire Protocol:** AWP 1 (44-byte framed header, CRC32-checked, JSON
  payloads), unchanged in v0.6.0. The cluster health section (with additive
  fail-stop diagnostics fields), the optional additive `retryable` error hint,
  and the `not_leader` error code are additive. See [PROTOCOL.md](PROTOCOL.md).
- **Aura Connector (tested):** 0.3.x
- **Cluster mode:** optional, off by default. Single-node cluster, plus a
  controlled, **experimental multi-node server preview** gated by two opt-ins;
  single-node mode remains the recommended production path. v0.6.0 improves the
  preview's fail-stop recovery ergonomics but is **not** production HA. See
  [CLUSTERING.md](CLUSTERING.md).

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

- Single-node mode is the recommended production path. v0.5.0 added a controlled,
  experimental multi-node server preview (off by default, gated by two opt-ins),
  v0.5.x hardened it, and v0.6.0 improves its fail-stop recovery ergonomics and
  diagnostics; it is **not** production multi-node clustering. It has no
  production automatic failover, no linearizable follower reads, no distributed
  transactions, no dynamic membership, and no sharding or multi-region.
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
