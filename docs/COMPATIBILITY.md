# AuraDB Compatibility Matrix

This document records what AuraDB v1.4.0 implements and how it interoperates with
the Aura Connector client library and the Aura Wire Protocol (AWP). AuraDB v1.4.0
continues the **v1 single-node production line** with a release focused on production
operability and search quality: on top of the v1.3 surface it adds a single-node
production drill harness (backup/restore rehearsal, rollback drill, disk-space
preflight, and a safe injected I/O-error drill, with a machine-readable drill report)
and a search relevance evaluation toolchain (`auradb search eval`) that computes
MRR@k, NDCG@k, and Recall@k over a small committed relevance fixture, with BM25
`k1`/`b` evaluation guidance, a hybrid calibration harness, and a `vector_exact`
evaluation mode — while remaining a production single-node deployment configured with
auth, TLS, backups, monitoring, and the documented runbooks. These additions are
operability and evaluation **tooling**; they introduce **no new wire, storage, or
query-engine surface**. Multi-node static clustering remains an HA candidate preview,
**not a production HA guarantee**. **AWP 1, storage format v2, and the index snapshot
format version (1) are unchanged**, so v1.3.x and earlier data open with no required
rebuild. Exact vector search remains the default and the correctness baseline;
approximate (HNSW) vector search is available as an **opt-in preview** (the graph is
never persisted; it rebuilds in memory from the exact vectors on use), **not
production ANN**. The relevance fixtures are regression signals for the shipped
datasets, **not universal benchmarks or guaranteed relevance**.
See [SEARCH_AND_RANKING.md](SEARCH_AND_RANKING.md),
[SUPPORT_POLICY.md](SUPPORT_POLICY.md),
[HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md), and the
[v1.0 decision checklist](V1_0_DECISION_CHECKLIST.md).

**Aura Wire Protocol 1 is frozen for v1.** AuraDB v1.1.0 uses Aura Wire Protocol 1.
AWP 1 is the stable v1 wire protocol. The v1.1.0 search and ranking clauses
(`text_search`, `hybrid`) and their response score fields are additive: older
clients omit and ignore them, so the wire revision is unchanged. AuraDB v1.x will preserve AWP 1 compatibility
unless a security or correctness issue requires a documented compatibility break.
The `not_leader` error frame carries an additive structured `not_leader` object
(leader client address, leader/current node ids, term, role, and a usable
`leader_hint`) — byte-for-byte the same as v0.7.0 — that older clients ignore,
alongside the additive cluster health and `retryable` fields. Aura Connector 0.4.x
reads the new fields; Aura Connector 0.3.x stays fully compatible (it simply does
not consume them).

**Storage format v2 is frozen for v1.** AuraDB v1.1.0 uses storage format v2.
Storage format v2 is the stable v1 single-node storage format. BM25 length
statistics persist additively inside the existing index snapshot format (rebuilt
safely on open from pre-v1.1.0 snapshots); the storage log format is unchanged. AuraDB v1.x will
preserve storage format v2 compatibility unless a safety, corruption, or security
issue requires a documented migration. The on-disk format is unchanged from
v0.4.x. Single-node mode remains the recommended production mode.

| AuraDB | Aura Connector | Protocol | Status |
| ------ | -------------- | -------- | ------ |
| 1.4.0  | 0.8.0          | AWP 1    | Supported, recommended (production operability and search quality over 1.3.x: adds the single-node production drill harness (`scripts/smoke_single_node_production_drills.sh`) — backup/restore rehearsal, rollback drill, disk-space preflight, and a safe injected I/O-error drill with a machine-readable drill report (the disk-full drill is preflight/injected-failure style, it does not actually fill the disk) — and the `auradb search eval` relevance evaluation toolchain (MRR@k, NDCG@k, Recall@k over a small committed fixture; `bm25`/`hybrid`/`vector_exact` modes; BM25 `k1`/`b` guidance; hybrid calibration). These are operability and evaluation tooling with no new wire, storage, or query-engine surface; AWP 1, storage format v2, and index snapshot format version 1 unchanged; single-node production line; multi-node HA candidate preview, not production HA; exact vector search remains the default and correctness baseline, with the opt-in approximate (HNSW) vector preview — never persisted, rebuilt in memory on use, not production ANN; relevance fixtures are regression signals, not universal benchmarks). Connector 0.8.x; 0.7.x/0.6.x/0.5.x remain supported for existing features (no wire change). |
| 1.3.1  | 0.7.0          | AWP 1    | Supported (patch over 1.3.0 for release-smoke correctness: fixes the cluster search-analytics smoke (`scripts/smoke_cluster_search_analytics.sh`) to resolve the leader by each node's self-reported role and to wait for a genuine leader change during the failover drill instead of grepping an address token and accepting a stale stopped leader; no engine, protocol, storage, query, or connector behavior changes; AWP 1, storage format v2, and index snapshot format version 1 unchanged; single-node production line; multi-node HA candidate preview, not production HA; exact vector search remains the default and correctness baseline, with the opt-in approximate (HNSW) vector preview — never persisted, rebuilt in memory on use, not production ANN). Connector 0.7.x; 0.6.x remains supported (backward compatible with 0.6.1) for existing features. |
| 1.3.0  | 0.7.0          | AWP 1    | Supported, recommended (query ergonomics, vector-preview durability, and query observability over 1.2.x: GROUP BY aggregations (additive `group_by`/`group_limit`), EXPLAIN ANALYZE query-profile fields, durable approximate-preview lifecycle metadata with an `ann_fallback` exact/error policy, and the `auradb vector eval` recall/latency harness; single-node production line; multi-node HA candidate preview, not production HA; AWP 1, storage format v2, and index snapshot format version 1 unchanged; exact vector search remains the default and correctness baseline, with the opt-in approximate (HNSW) vector preview — never persisted, rebuilt in memory on use, not production ANN). Connector 0.7.x; 0.6.x remains supported (backward compatible with 0.6.1) for existing features. |
| 1.2.1  | 0.6.1          | AWP 1    | Supported (conformance and documentation hardening over 1.2.0: adds live over-the-wire conformance scripts for facets, aggregations, ranked pagination, and cooperative query timeouts, and refreshes support/production docs; no new database or query features; AWP 1, storage format v2, and the v1.2.0 feature set unchanged; exact vector search remains the default and correctness baseline, with the opt-in approximate (HNSW) vector preview — in-memory/rebuilt, not production ANN). Connector 0.6.0 remains supported; 0.5.x remains supported for pre-1.2 features. |
| 1.2.0  | 0.6.0          | AWP 1    | Supported (query ergonomics release: aggregations, terms facets, cooperative query timeouts; single-node production line; multi-node HA candidate preview, not production HA; aggregate request and per-query `timeout_ms` are additive Query IR; AWP 1 and storage format v2 unchanged; exact vector search is the default and correctness baseline, with an opt-in approximate (HNSW) vector preview — in-memory/rebuilt, not production ANN). Connector 0.5.x remains supported for pre-1.2 features. |
| 1.1.0  | 0.5.0          | AWP 1    | Supported (search and ranking release: BM25 ranked full-text, hybrid text+vector; single-node production line; multi-node HA candidate preview, not production HA; new clauses are additive Query IR/response fields; AWP 1 and storage format v2 unchanged; exact vector search only, no ANN) |
| 1.0.1  | 0.4.1          | AWP 1    | Supported (first production patch on the v1.0 single-node production line; multi-node HA candidate preview, not production HA; no new config, architecture, or semantics over 1.0.0; connector unchanged; AWP 1 and storage format v2 frozen for v1) |
| 1.0.0  | 0.4.1          | AWP 1    | Supported (single-node production release; multi-node HA candidate preview, not production HA; no new config, architecture, or semantics over 0.9.2; connector unchanged; AWP 1 and storage format v2 frozen for v1) |
| 0.9.2  | 0.4.1          | AWP 1    | Supported, recommended (final HA candidate stabilization; no new config, architecture, or semantics over 0.9.1; connector unchanged; wire payload, storage format (v2), and semantics identical to 0.9.1) |
| 0.9.1  | 0.4.1          | AWP 1    | Supported (HA release-candidate stabilization of the 0.9.0 candidate; adds the optional, backward-compatible `[cluster] advertise_client_addr` field; connector unchanged; wire payload, storage format (v2), and semantics identical to 0.9.0) |
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

- **AuraDB:** 1.1.0
- **Storage format:** v2 (commit-timestamped MVCC version chains), **frozen for
  v1** and unchanged from v0.4.x. AuraDB v1.x preserves storage format v2 unless a
  safety, corruption, or security issue requires a documented migration. A v1
  (≤ 0.2.x) data directory is migrated to v2 transparently on first open; an
  unknown future format is rejected. See [UPGRADING.md](UPGRADING.md).
- **Aura Wire Protocol:** AWP 1 (44-byte framed header, CRC32-checked, JSON
  payloads), **frozen for v1** and unchanged since v0.8.0. AuraDB v1.x preserves
  AWP 1 unless a security or correctness issue requires a documented compatibility
  break. The cluster health section, the optional
  additive `retryable` error hint, the `not_leader` error code, and the
  additive structured `not_leader` object on the error frame are all additive and
  ignored by older clients. See [PROTOCOL.md](PROTOCOL.md).
- **Aura Connector (tested):** 0.7.0 (supported 0.7.x; 0.6.x remains supported and backward
  compatible with 0.6.1 for existing features; 0.4.x connects for non-search operations)
- **Cluster mode:** optional, off by default. Single-node cluster, plus a
  controlled multi-node server **HA candidate preview** gated by two opt-ins;
  single-node mode remains the recommended production path. Multi-node static
  clustering in v1.0 remains an HA candidate preview — strong release-candidate
  evidence, but **not** a production HA guarantee. The optional
  `[cluster] advertise_client_addr` field (this node's own client-facing address,
  reported as the leader hint while it leads) is **additive and backward
  compatible** — a config that omits it behaves exactly as before. See
  [CLUSTERING.md](CLUSTERING.md) and [CONFIGURATION.md](CONFIGURATION.md).

## v1.1 support matrix

This summarizes the support level of each deployment mode and capability. The
authoritative policy is [SUPPORT_POLICY.md](SUPPORT_POLICY.md).

| Mode / capability | Status | Supported in v1.0 | Production use | Notes |
| ----------------- | ------ | ----------------- | -------------- | ----- |
| Single-node, local loopback bind | Stable | Yes | Dev / local | Auth may be disabled for local dev only |
| Single-node, network bind with auth + TLS | Stable | Yes | **Yes (recommended)** | The supported production mode; run the production runbook |
| Docker single-node (base image) | Stable | Yes | Dev / local | Base image binds all interfaces with `--allow-insecure-bind`; dev only |
| Docker secure Compose | Stable | Yes | **Yes** | Auth + TLS, non-root, read-only root fs, no committed secret |
| Static multi-node cluster | Preview | Preview only | No | HA candidate preview, not production HA |
| Public peer networking | Preview | Preview only | No | Requires peer TLS + token; off by default |
| Backup / restore | Stable | Yes | **Yes** | `dump` → `backup verify` → `restore` → `check` |
| Upgrade from v0.x | Stable | Yes | **Yes** | Backup first; `check` before and after; see [UPGRADING.md](UPGRADING.md) |
| Aura Connector 0.5.x | Stable | Yes | **Yes** | v0.5.0 recommended; 0.4.x connects for non-search operations |
| AWP 1 | Frozen for v1 | Yes | **Yes** | Preserved across v1.x unless security/correctness break |
| Storage format v2 | Frozen for v1 | Yes | **Yes** | Preserved across v1.x unless safety/corruption/security migration |
| Exact vector search | Stable | Yes | **Yes** | `cosine` / `euclidean` / `dot_product`; default and correctness baseline |
| Approximate (HNSW) vector search | Preview | Preview only | No | Opt-in (v1.2.0); in-memory/rebuilt, not persisted/incremental; not production ANN |
| Tokenized full-text search | Stable | Yes | **Yes** | Boolean `contains_text`; term-frequency ranking |
| BM25 ranked full-text search | Stable | Yes (new in v1.1.0) | **Yes** | `text_search` clause; Okapi BM25 |
| Hybrid text + vector search | Stable | Yes (new in v1.1.0) | **Yes** | `hybrid` clause; weighted-sum / RRF fusion |

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
| aggregate (count/min/max/avg, terms facets) | Yes | `aggregate` read; `avg` and `group_by` added in v1.3.0 |
| group by (single scalar field) | Yes | additive `group_by`/`group_limit` on `aggregate` (v1.3.0) |
| explain | Yes | reports strategy, the index used, estimated rows/cost, and vector `vector_mode` |
| explain analyze | Yes | execution metrics via the raw IR `"analyze": true` flag (no new opcode); v1.3.0 adds additive `plan_id`/`deadline_ms`/`timeout_checked` |
| migration estimate | Yes | |

## Supported schema features

- Fields: `uuid`, `string`, `int`, `float`, `bool`, `timestamp`, `document`,
  `bytes`, `vector { dim }`.
- Field flags: `primary_key`, `unique`, `nullable`, `indexed`.
- Named indexes: `document_path` (dotted path equality), `full_text` (tokenized
  inverted index on a string field).
- Relationships: to-one and to-many links with `restrict` / `set_null` on delete.

## Supported vector operations

- Exact nearest-neighbour search over a fixed-dimension vector field (the default
  and the correctness baseline).
- Metrics: `cosine`, `euclidean`, `dot_product`.
- Approximate nearest neighbour (HNSW) is available as an **opt-in preview** (v1.2.0):
  the graph is never persisted; it rebuilds in memory from the exact vectors on use, and
  it is **not production ANN**. Exact search remains the correctness baseline. v1.3.0
  adds durable per-field preview lifecycle metadata (an additive index-snapshot field;
  the graph is still not persisted), an `ann_fallback` policy (`exact` default / `error`)
  for when the preview is unavailable, the `ANN_PREVIEW_MIN_VECTORS = 16` threshold, a
  `vector_mode` field in EXPLAIN, and the `auradb vector eval` recall/latency harness.

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

- Single-node mode is the recommended production path. The controlled multi-node
  server cluster is an **HA candidate preview** (off by default, gated by two
  opt-ins); it is **not** production multi-node clustering. It has no production
  automatic failover, no linearizable follower reads, no distributed transactions,
  no dynamic membership, and no sharding or multi-region.
- AuraDB implements single-node snapshot isolation with optimistic write
  conflict detection. It is not serializable isolation (it does not prevent
  write-skew).
- Vector search is exact by default (the correctness baseline). An opt-in
  approximate (HNSW) preview is available — in-memory/rebuilt, not
  persisted/incremental, and not production ANN.
- The legacy `contains_text` predicate is tokenized boolean-AND matching with
  term-frequency ranking. BM25 ranked full-text (`text_search`) and hybrid
  text+vector (`hybrid`) search are implemented as of v1.1.0; see
  [SEARCH_AND_RANKING.md](SEARCH_AND_RANKING.md).
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
