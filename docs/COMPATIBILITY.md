# AuraDB Compatibility Matrix

This document records what AuraDB v0.9.1 implements and how it interoperates with
the Aura Connector client library and the Aura Wire Protocol (AWP). v0.9.1 is an
**HA release candidate for the controlled static-cluster preview, not a
production HA guarantee** — it stabilizes the v0.9.0 candidate: it adds an
optional, additive `[cluster] advertise_client_addr` field so a leader can name
its own client address in the `not_leader` hint and cluster status/health,
extends snapshot/compaction and connector-leader-change test coverage across a
leader change, and sharpens the HA candidate smoke and connector conformance
diagnostics — it adds no new cluster architecture and changes no semantics,
storage format, or wire protocol, and keeps the v0.8.x cluster-preview ergonomics
(off by default). See
[HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md). The v0.8.x candidate it builds
on is a hardening, validation, and operability release — a production-readiness
candidate for single-node and a stronger cluster preview — coordinated with the
unchanged Aura Connector v0.4.1. Single-node mode remains the recommended production mode. It
keeps **AWP 1** unchanged and makes no incompatible protocol change: the
`not_leader` error frame carries an additive structured `not_leader` object
(leader client address, leader/current node ids, term, role, and a usable
`leader_hint`) — byte-for-byte the same as v0.7.0 — that older clients ignore,
alongside the existing additive cluster health and `retryable` fields. Aura
Connector 0.4.x reads the new fields; Aura Connector 0.3.x stays fully compatible
(it simply does not consume them). The on-disk **storage format is unchanged**
from v0.4.x.

| AuraDB | Aura Connector | Protocol | Status |
| ------ | -------------- | -------- | ------ |
| 0.9.1  | 0.4.1          | AWP 1    | Supported, recommended (HA release-candidate stabilization of the 0.9.0 candidate; adds the optional, backward-compatible `[cluster] advertise_client_addr` field; connector unchanged; wire payload, storage format (v2), and semantics identical to 0.9.0) |
| 0.9.0  | 0.4.1          | AWP 1    | Supported (HA release candidate for the controlled static-cluster preview, not production HA; connector unchanged; wire payload, storage format (v2), and semantics identical to 0.8.x) |
| 0.8.1  | 0.4.1          | AWP 1    | Supported (stabilization patch over 0.8.0; connector unchanged; wire payload, storage format, and semantics identical to 0.8.0) |
| 0.8.0  | 0.4.1          | AWP 1    | Supported (AuraDB-focused hardening release; connector unchanged; wire payload identical to 0.7.x; storage format unchanged) |
| 0.7.1  | 0.4.1          | AWP 1    | Supported, recommended (clearer `AuraNotLeaderError` messages, secure-by-default redirect, transaction-redirect docs; identical wire payload to 0.7.0) |
| 0.7.1  | 0.3.x / 0.4.0  | AWP 1    | Supported (additive `not_leader` payload; older clients route the leader manually or via 0.4.0 helpers) |
| 0.7.0  | 0.4.x          | AWP 1    | Supported (native AuraDB backend; structured `not_leader` payload + connector cluster ergonomics) |
| 0.6.2  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; additive `leader_changes` diagnostics field; repeated-chaos / larger-state recovery hardening) |
| 0.6.1  | 0.3.x          | AWP 1    | Supported (native AuraDB backend; additive snapshot/lag diagnostics fields; multi-arch image; preview hardening) |
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

- **AuraDB:** 0.9.1
- **Storage format:** v2 (commit-timestamped MVCC version chains), unchanged from
  v0.4.x. A v1 (≤ 0.2.x) data directory is migrated to v2 transparently on first
  open; an unknown future format is rejected. See [UPGRADING.md](UPGRADING.md).
- **Aura Wire Protocol:** AWP 1 (44-byte framed header, CRC32-checked, JSON
  payloads), unchanged in v0.8.0. The cluster health section, the optional
  additive `retryable` error hint, the `not_leader` error code, and the
  additive structured `not_leader` object on the error frame are all additive and
  ignored by older clients. See [PROTOCOL.md](PROTOCOL.md).
- **Aura Connector (tested):** 0.4.1 (also compatible with 0.4.0 and 0.3.x single-node)
- **Cluster mode:** optional, off by default. Single-node cluster, plus a
  controlled, **experimental multi-node server preview** gated by two opt-ins;
  single-node mode remains the recommended production path. v0.7.x improves the
  preview's connector-facing error ergonomics and v0.8.0 hardens preview recovery
  testing, but it is **not** production HA. v0.9.1 adds an optional
  `[cluster] advertise_client_addr` field (this node's own client-facing address,
  reported as the leader hint while it leads); it is **additive and backward
  compatible** — a config that omits it behaves exactly as in v0.9.0. See
  [CLUSTERING.md](CLUSTERING.md) and [CONFIGURATION.md](CONFIGURATION.md).

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

- **Test date:** 2026-06-08
- **CI workflows:** `ci.yml` (build, fmt, clippy, test, benchmark compilation),
  `conformance.yml` (Python AWP conformance: auth disabled, auth enabled, and
  TLS, plus the Aura Connector smoke and conformance suites), `cluster.yml`
  (loopback cluster + Aura Connector 0.4.x cluster conformance: leader smoke,
  leader conformance, and leader/follower cluster ergonomics), `security.yml`
  (cargo audit and deny), `docker.yml`, `release.yml`.
- **Conformance harness:** `tests/conformance/python/run_conformance.py`.
- **Connector smoke:** `tests/conformance/python/run_connector_smoke.py`.
