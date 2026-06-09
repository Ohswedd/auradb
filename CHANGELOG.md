# Changelog

All notable changes to AuraDB are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project uses
[Semantic Versioning](https://semver.org/).

## [1.3.1] - 2026-06-09

**Release-smoke correctness — single-node production line, multi-node HA candidate preview.**
AuraDB v1.3.1 is a patch release that fixes the cluster search-analytics release smoke
(`scripts/smoke_cluster_search_analytics.sh`) and changes **no** engine, protocol, storage,
query, or connector behavior. The smoke previously resolved the cluster leader by grepping the
first `127.0.0.1:<port>` token, which could match the queried seed's own address and biased the
hardcoded `7171`/node1 port, so the run could fail whenever node2 or node3 won the Compose
election; its failover drill could also accept a stale stopped leader. The fixed smoke resolves
the leader by each node's **self-reported role** and **waits for a genuine leader change**
(excluding the stopped port) during the drill. The engine and image were already correct — this
was a smoke/test-harness bug only. **Aura Wire Protocol 1, storage format v2, the index snapshot
format version (1), and the entire v1.3.0 feature set stay unchanged.** Aura Connector **v0.7.0**
(compatible 0.7.x; 0.6.x still supported for existing features; backward compatible with 0.6.1)
remains the paired client. The v1.3.0 tag is **not** moved. See
[docs/V1_3_1_RELEASE_NOTES.md](docs/V1_3_1_RELEASE_NOTES.md) and
[docs/COMPATIBILITY.md](docs/COMPATIBILITY.md).

### Fixed

- **Cluster search-analytics release smoke leader resolution.** `find_leader_addr` now polls
  each node's own `cluster status` and selects the host port whose node reports `role=Leader`,
  rather than grepping a `leader_client_addr` token that could match the queried seed. The
  bounded leader-change drill stops the current leader, waits until a different reachable node
  reports `role=Leader` (excluding the stopped port so a stale survivor pointing at the dead
  node is rejected), re-runs the search/facet/pagination/group-by checks under the new leader,
  and restores quorum after rejoin. Plain `case` port↔service lookups replace any associative
  arrays, so the script runs on stock macOS bash 3.2. The script header still states it is the
  experimental multi-node preview and an HA *candidate* drill — **not production HA proof**.

### Unchanged

- **Aura Wire Protocol 1**, **storage format v2**, and the **index snapshot format version
  (1)** are frozen; no wire or on-disk format change. v1.3.0, v1.2, v1.1, and v1.0 data open
  unchanged with no required rebuild.
- The entire v1.3.0 feature set (GROUP BY aggregations, EXPLAIN ANALYZE query-profile fields,
  durable approximate-vector preview metadata with an `ann_fallback` policy, the `auradb vector
  eval` harness, and all earlier v1.x query features) is carried forward byte-for-byte. No
  engine, query, storage, replication, or connector behavior changes.
- Single-node production support; multi-node HA candidate preview (not production HA).
- **Exact vector search remains the correctness baseline.** Approximate (HNSW) vector search
  is an **opt-in preview**, not production ANN.

### Known limitations

Honest limitations carried by this release (unchanged scope boundaries):

- **Multi-node is an HA candidate preview, not production HA.** No production automatic
  failover, no linearizable follower reads (follower reads/search are eventually consistent),
  no distributed transactions, and no dynamic membership, sharding, or multi-region.
  Single-node remains the recommended production mode.
- **Approximate (HNSW) vector search is an opt-in preview, not production ANN.** The graph is
  in-memory and rebuilt from the exact vectors (never persisted; not incremental). Exact
  vector search remains the default and the correctness baseline.
- **The cluster search-analytics smoke is a controlled single-host preview drill.** It
  exercises a static three-node Compose cluster on one host and is **not** production HA proof.

## [1.3.0] - 2026-06-09

**Query ergonomics, vector-preview durability, and query observability — single-node
production line, multi-node HA candidate preview.** AuraDB v1.3.0 adds GROUP BY aggregations
and EXPLAIN ANALYZE query-profile fields to the single-node production line, and matures the
opt-in approximate-vector preview with durable lifecycle metadata and an explicit
exact-fallback policy — without changing the production support claim. Single-node remains the
recommended production mode; multi-node static clustering remains an HA candidate preview — not
production HA. **Aura Wire Protocol 1, storage format v2, and the index snapshot format
version (1) stay frozen**: GROUP BY is additive Query IR, the approximate-preview lifecycle
metadata is an additive index-snapshot field (the graph itself is still never persisted), and
the profile fields are additive ANALYZE JSON. Aura Connector **v0.7.0** (compatible 0.7.x;
0.6.x still supported for existing features; backward compatible with 0.6.1) is the paired
client. See [docs/V1_3_RELEASE_NOTES.md](docs/V1_3_RELEASE_NOTES.md),
[docs/QUERY_ENGINE.md](docs/QUERY_ENGINE.md), and [docs/VECTORS.md](docs/VECTORS.md).

### Added

- **GROUP BY aggregations.** The `aggregate` read request gains an additive `group_by` clause:
  a single scalar field bucketing the matched set, with per-group `count`, `min`, `max`, and
  the new `avg` metric. Groups ride the same matched set as facets/metrics, so they compose
  with filters and BM25 search-candidate scoping; null/missing group keys are excluded;
  ordering is deterministic (descending count, then ascending key); an optional `group_limit`
  (default 1000) truncates while `group_count_total` reports the full distinct-group count.
  `avg` considers only `Int`/`Float` values and yields null when a group has none; a
  non-scalar or unknown group field is rejected with `invalid_request`.
- **Approximate-vector (HNSW) preview durability and exact fallback.** The index snapshot now
  records additive per-field lifecycle metadata (field, dimension, vector count, generation
  marker) so the preview's status is visible across restarts; the approximate graph itself is
  still never persisted and rebuilds in memory from the exact vectors on first use. A new
  `ann_fallback` policy (`exact` default / `error`) governs queries when the preview is
  unavailable (for example below `ANN_PREVIEW_MIN_VECTORS = 16` indexed vectors). `EXPLAIN`
  reports the resolved `vector_mode` (`exact`, `ann_preview`, or `exact_fallback`).
- **`auradb vector eval` recall/latency harness.** A new operator command measures the
  approximate preview's recall@k and latency against the exact baseline over a deterministic
  query set (`--data-dir`, `--collection`, `--field`, `--queries`, `--k`, `--metric`,
  `--ef-search`, `--json`). The JSON report carries `collection`, `field`, `metric`,
  `queries`, `k`, `ef_search`, `mean_recall_at_k`, `min_recall_at_k`, `exact_latency_ms_p50`,
  and `ann_latency_ms_p50`; the query vectors are never echoed. Numbers are dataset- and
  machine-specific, never a universal claim.
- **EXPLAIN ANALYZE query-profile fields.** The ANALYZE output gains additive
  `plan_id` (deterministic per plan shape), `deadline_ms` (the cooperative deadline in effect,
  or null), and `timeout_checked` fields for debugging, alongside the existing measured counts
  and timings. The query payload is never echoed into the plan.

### Unchanged

- **Aura Wire Protocol 1**, **storage format v2**, and the **index snapshot format version
  (1)** are frozen; no wire or on-disk format change. v1.2, v1.1, and v1.0 data open unchanged
  with no required index rebuild (the approximate graph rebuilds in memory on first use).
- The v1.2 query feature set (aggregations, terms facets, cooperative query timeouts, ranked
  pagination, opt-in HNSW vector preview) is carried forward.
- Single-node production support; multi-node HA candidate preview (not production HA).
- **Exact vector search remains the correctness baseline.** Approximate (HNSW) vector search
  is an **opt-in preview**, not production ANN.

### Known limitations

Honest limitations carried by this release (unchanged scope boundaries):

- **Multi-node is an HA candidate preview, not production HA.** No production automatic
  failover, no linearizable follower reads (follower reads/search are eventually consistent),
  no distributed transactions, and no dynamic membership, sharding, or multi-region.
  Single-node remains the recommended production mode.
- **Approximate (HNSW) vector search is an opt-in preview, not production ANN.** The graph is
  never persisted; it rebuilds in memory from the exact vectors on use. Only the lifecycle
  metadata is durable. Below `ANN_PREVIEW_MIN_VECTORS = 16` the preview is unavailable and the
  `ann_fallback` policy applies. Exact vector search remains the default and the correctness
  baseline.
- **`auradb vector eval` does not emit a candidate-count average.** Per-query HNSW
  candidates-visited is not yet surfaced through the query result, so `candidate_count_avg` is
  not in the report; recall and latency are. The numbers are dataset- and machine-specific.
- **Query timeouts are cooperative, not preemptive.** Reads poll the deadline on their
  candidate/scan loop, so cancellation is "soon after" the deadline rather than instantaneous;
  the EXPLAIN ANALYZE `deadline_ms`/`timeout_checked` fields report this cooperative deadline.

## [1.2.1] - 2026-06-09

**Conformance and documentation hardening — single-node production line, multi-node HA
candidate preview.** AuraDB v1.2.1 is a hardening release: it adds **no** database or query
features over v1.2.0 and changes no product behavior. It adds live over-the-wire conformance
scripts that exercise the v1.2 features (facets, aggregations, ranked pagination, cooperative
query timeouts) end-to-end through the Aura Connector, wires them into the conformance
workflow, and refreshes the support and production documentation to enumerate the v1.2
feature set honestly. **Aura Wire Protocol 1, storage format v2, and the v1.2.0 feature set
stay unchanged.** Aura Connector **v0.6.1** (compatible 0.6.x; 0.6.0 still supported; 0.5.x
still supported for existing features) is the paired client. See
[docs/V1_2_1_RELEASE_NOTES.md](docs/V1_2_1_RELEASE_NOTES.md) and
[docs/CONFORMANCE.md](docs/CONFORMANCE.md).

### Added

- **Live v1.2 conformance scripts.** New over-the-wire conformance harnesses drive a running
  server through the Aura Connector: `run_connector_facets.py` (terms facets and
  count/min/max aggregations, including BM25-scoped facets), `run_connector_pagination.py`
  (ranked pagination by stable cursor token, duplicate-free pages, structured invalid-cursor
  rejection), and `run_connector_timeouts.py` (per-query `timeout_ms` acceptance, the
  `query_timeout` error shape, and connection survival after a timeout). Cluster variants
  (`run_connector_facets_cluster.py`, `run_connector_pagination_cluster.py`,
  `run_connector_timeouts_cluster.py`) drive the same features through a leader and document
  eventually-consistent follower-read behavior.
- **CI wiring.** The conformance workflow runs the new single-node scripts against a live
  server using the paired connector; the cluster variants are documented as operator-run
  (their leader-change steps require stopping a node) in
  [docs/CONFORMANCE.md](docs/CONFORMANCE.md) and [docs/TESTING.md](docs/TESTING.md).

### Documentation

- `docs/SUPPORT_POLICY.md` and `docs/PRODUCTION_READINESS.md` now enumerate the full v1.2
  single-node production feature set (BM25/hybrid/exact-vector search, aggregations, terms
  facets, ranked pagination, cooperative query timeouts, backup/restore, auth/TLS,
  monitoring, diagnostics) and clearly separate production-supported from preview (opt-in
  HNSW/ANN, multi-node HA candidate) and explicitly unsupported (production ANN, production
  HA, dynamic membership, linearizable follower reads, distributed transactions, sharding,
  multi-region).
- Compatibility, conformance, and release docs record the 1.2.1 ↔ 0.6.1 pairing and the live
  conformance coverage.

### Unchanged

- **Aura Wire Protocol 1** and **storage format v2** are frozen; no wire or on-disk changes.
  v1.2.0, v1.1, and v1.0 data open unchanged with no required rebuild.
- The v1.2.0 query feature set (aggregations, terms facets, cooperative query timeouts,
  ranked pagination, opt-in HNSW vector preview) is carried forward byte-for-byte.
- Single-node production support; multi-node HA candidate preview (not production HA).
- **Exact vector search remains the correctness baseline.** Approximate (HNSW) vector search
  is an **opt-in preview**, not production ANN.

### Known limitations

Honest limitations carried by this release (unchanged scope boundaries):

- **Multi-node is an HA candidate preview, not production HA.** No production automatic
  failover, no linearizable follower reads (follower reads/search are eventually consistent),
  no distributed transactions, and no dynamic membership, sharding, or multi-region.
  Single-node remains the recommended production mode.
- **Approximate (HNSW) vector search is an opt-in preview, not production ANN.** The graph is
  in-memory and rebuilt from the exact vectors (never persisted; not incremental). Exact
  vector search remains the default and the correctness baseline.
- **Query timeouts are cooperative, not preemptive.** Reads poll the deadline on their
  candidate/scan loop, so cancellation is "soon after" the deadline rather than instantaneous.
- **Ranked-pagination cursor stability under concurrent writes.** Vector cursors are
  duplicate-free across concurrent writes; BM25/hybrid ranked pagination is stable only when
  paged inside a transaction snapshot. The connector iterates cursors internally; resuming a
  ranked page from an externally held cursor token is not part of the public API.

## [1.2.0] - 2026-06-09

**Query ergonomics and operational hardening — single-node production line, multi-node HA
candidate preview.** AuraDB v1.2.0 adds aggregations, terms facets, and cooperative query
timeouts to the single-node production line without changing the production support claim.
Single-node remains the recommended production mode; multi-node static clustering remains an
HA candidate preview — not production HA. **Aura Wire Protocol 1 and storage format v2 stay
frozen**: the aggregate request and the per-query `timeout_ms` are additive Query IR, and no
on-disk format changes (aggregations/facets read the existing records and indexes). Aura
Connector **v0.6.0** (compatible 0.6.x; 0.5.x still supported for existing features) is the
paired client. See [docs/V1_2_RELEASE_NOTES.md](docs/V1_2_RELEASE_NOTES.md) and
[docs/QUERY_ENGINE.md](docs/QUERY_ENGINE.md).

### Added

- **Aggregations and terms facets.** A new `aggregate` read request computes `count`/`min`/
  `max` metrics and terms facets over a collection. Facets and metrics share one matched set
  (a filtered scan, or the BM25 candidate set when a `text_search` clause is present). A
  terms facet over an equality-indexed field with no residual filter is served from index
  posting-list lengths; other shapes fall back to an honest scan, reported per-facet as
  `used_index`. Buckets order deterministically by descending count then ascending value.
- **Cooperative query timeouts.** A configurable default deadline (`[limits]
  max_query_time_ms`, default 30000ms; `0` disables) bounds every read; a per-query
  `timeout_ms` may lower but not raise it. Scan, BM25, hybrid, exact-vector, and
  aggregate/facet reads poll the deadline and return a structured `query_timeout` error when
  exceeded. The session and connection remain usable after a timeout.
- **Approximate vector search (HNSW) preview — opt-in.** A real HNSW approximate index is
  available per query via the additive `vector_ann` option (`m`/`ef_construction`/`ef_search`)
  on a vector clause. Exact search remains the default and correctness baseline; the graph is
  built in memory from the exact vectors (never persisted; storage format v2 unchanged) and
  rebuilt when vectors change. Validated by recall tests against the exact baseline,
  determinism, EXPLAIN (`approximate`/`ef_search`), and parameter / dimension-mismatch
  rejection (payload not echoed). Advertised as `approximate_vector_search_preview` with an
  `ann_preview_queries_total` metric. **Not production ANN.**
- **Exact vector search optimization.** Exact nearest-neighbour selection uses a bounded
  top-k heap (`O(n log k)`) instead of a full sort, with provably identical results (ranking
  key and NaN handling unchanged; verified by a full-sort reference test).
- **Stable ranked pagination.** The additive `search_page` read request (and
  `Engine::search_page`) pages ranked search (BM25/hybrid/vector) by opaque, bounded keyset
  cursor tokens that carry no query payload or secrets and reject replay against a different
  query. Vector cursors are duplicate-free across concurrent writes; BM25/hybrid are stable
  when paged inside a transaction snapshot. Advertised as the `ranked_pagination` capability
  and surfaced in the connector as `QueryBuilder.search_pages(page_size=...)`.
- **Capabilities:** `aggregations_and_facets` and `query_timeouts` are advertised by
  `auradb compatibility`, which also lists `facets`, `aggregations`, and `query_timeouts`
  under search features.
- **Observability:** `query_timeouts_total`, `facets_queries_total`,
  `aggregation_queries_total`, and `ann_preview_queries_total` counters.
- **Benchmarks:** `benches/baseline/v1.2.0.json` adds `aggregate_count`, `facet_terms`,
  `vector_ann_preview` (next to `vector_exact_nearest`), and `ranked_pagination_first_page`.
  The numbers are same-machine, release-build regression signals — the approximate preview is
  honestly slower than exact at small scale (see [docs/BENCHMARKS.md](docs/BENCHMARKS.md)).
- **Conformance:** five v1.2.0 scenarios in the over-the-wire suite — `aggregate_count_min_max`,
  `terms_facet_index_backed`, `search_facet_bm25`, `vector_ann_preview`, and
  `ranked_pagination_search_page` (34/34 scenarios pass in-process).

### Unchanged

- **Aura Wire Protocol 1** and **storage format v2** are frozen; all additions are additive
  Query IR / response fields with no on-disk format change. v1.1 and v1.0 data open
  unchanged with no required rebuild.
- Single-node production support; multi-node HA candidate preview (not production HA).
- **Exact vector search remains the correctness baseline.** Approximate (HNSW) vector search
  is an **opt-in preview**, not production ANN.

### Not implemented in this release

Honest scope — these v1.2.0 theme items were **not** built and are **not** claimed anywhere:
search/vector operational checks beyond the v1.1 validations; production-grade ANN
(persisted/incremental HNSW graphs and ANN-specific `index check` / `stats analyze` — the
ANN preview rebuilds its in-memory graph from the exact vectors). Aggregations, terms facets,
query timeouts, ranked pagination, and the opt-in HNSW vector preview all landed end-to-end
(server + connector), and the query features are covered by multi-node preview tests on a
follower after replication, after a leader change, after a follower restart, and after a
snapshot install. See [docs/ROADMAP.md](docs/ROADMAP.md).

### Known limitations

Honest limitations carried by this release (unchanged scope boundaries):

- **Multi-node is an HA candidate preview, not production HA.** No production automatic
  failover, no linearizable follower reads (follower reads/search are eventually consistent),
  no distributed transactions, and no dynamic membership, sharding, or multi-region.
  Single-node remains the recommended production mode.
- **Approximate (HNSW) vector search is an opt-in preview, not production ANN.** The graph is
  in-memory and rebuilt from the exact vectors (never persisted; not incremental). Exact
  vector search remains the default and the correctness baseline.
- **Query timeouts are cooperative, not preemptive.** Reads poll the deadline on their
  candidate/scan loop, so cancellation is "soon after" the deadline rather than instantaneous.
- **Ranked-pagination cursor stability under concurrent writes.** Vector cursors are
  duplicate-free across concurrent writes; BM25/hybrid ranked pagination is stable only when
  paged inside a transaction snapshot.

## [1.1.0] - 2026-06-08

**Search and ranking — single-node production line, multi-node HA candidate preview.**
AuraDB v1.1.0 is the first larger post-1.0 release. It expands search and ranking for the
single-node production line and adds connector-native support, without changing the
production support claim. Single-node mode remains the recommended production mode;
multi-node static clustering remains an HA candidate preview — not production HA, no
production automatic failover, no production cluster readiness. **Aura Wire Protocol 1 and
storage format v2 stay frozen**: the new query clauses are additive Query IR and response
fields, and the BM25 length statistics persist additively in the existing index snapshot
format (rebuilt safely on open from older snapshots). Aura Connector **v0.5.0** (compatible
0.5.x) is the paired client. See [docs/V1_1_RELEASE_NOTES.md](docs/V1_1_RELEASE_NOTES.md)
and [docs/SEARCH_AND_RANKING.md](docs/SEARCH_AND_RANKING.md).

### Added

- **BM25 ranked full-text search.** A new `text_search` query clause ranks full-text
  indexed documents by Okapi BM25 relevance (document frequency, term frequency, and
  document-length normalization), with `or`/`and` term operators and tunable `k1`/`b`
  (defaults 1.2 / 0.75). The legacy `contains_text` boolean predicate is unchanged.
- **Hybrid text + vector ranking.** A new `hybrid` query clause fuses BM25 text relevance
  with exact vector similarity using `weighted_sum` (min-max normalized) or
  `reciprocal_rank_fusion`, with configurable per-signal weights. Results expose fused,
  text, and vector component scores and a 1-based rank.
- **Planner awareness** of ranked text and hybrid retrieval, with candidate estimates from
  full-text statistics and a stable, additive EXPLAIN / EXPLAIN ANALYZE shape (ranking mode,
  candidate sources and counts, fusion mode, weights).
- **CLI:** `auradb compatibility` now reports the search capabilities and Aura Connector
  0.5.0/0.5.x; `auradb index check` validates BM25 and vector index statistics; `auradb
  stats analyze` refreshes full-text statistics; new `auradb search explain [--analyze]`.
- **Observability:** `search_text_queries_total`, `search_hybrid_queries_total`,
  `search_vector_queries_total` counters and a `ranking_latency` histogram.

### Unchanged

- Exact vector search remains the correctness baseline. **Approximate (ANN/HNSW) vector
  search is not implemented in v1.1.0.**
- Aura Wire Protocol 1, storage format v2, auth, TLS, backup/restore, upgrade, Docker, and
  the single-node production support scope. Multi-node remains an HA candidate preview.

### Known limitations

- No approximate (ANN/HNSW) vector search; exact vector search is the correctness baseline.
- Multi-node is an HA candidate preview, **not production HA**: no production automatic
  failover, dynamic membership, sharding, multi-region, distributed transactions, or
  production read-consistency guarantee. Followers serve only eventually-consistent,
  non-linearizable reads — send reads (including search) to the leader for correctness.

## [1.0.1] - 2026-06-08

**First production patch — single-node production line, multi-node HA candidate
preview.** AuraDB v1.0.1 is the first patch on the v1.0 single-node production line:
a documentation, validation, and release-engineering patch. It carries forward
**all** v1.0.0 behavior and adds **no** new database or cluster architecture, changes
**no** semantics, and touches **no** on-disk or wire format. Single-node mode remains
the recommended production mode; multi-node static clustering remains an HA candidate
preview — not production HA, no production automatic failover, no production cluster
readiness. **Aura Wire Protocol 1 and storage format v2 stay frozen for v1**, and
Aura Connector v0.4.1 (and compatible 0.4.x) is the supported client. Known
limitations are unchanged. See [docs/V1_0_1_RELEASE_NOTES.md](docs/V1_0_1_RELEASE_NOTES.md),
[docs/SUPPORT_POLICY.md](docs/SUPPORT_POLICY.md), and
[docs/COMPATIBILITY.md](docs/COMPATIBILITY.md).

### Changed
- Bumped the workspace version to `1.0.1` (`auradb version` /
  `auradb compatibility`, `Cargo.toml`, `Cargo.lock`).
- Refreshed current-release version pointers and the support matrix to name v1.0.1
  across the README and docs; the single-node production support statement and the
  multi-node HA-candidate-preview disclaimer are unchanged in substance.
- Refreshed the benchmark baseline (`benches/baseline/v1.0.1.json`); benchmark
  numbers remain machine-specific and warn-only.

### Fixed
- Updated the developer-quickstart and secure-deployment Docker Compose examples
  (`docker-compose.yml`, `docker-compose.secure.yml`) to reference the current
  `ghcr.io/ohswedd/auradb:1.0.1` image. They had remained pinned to the obsolete
  `0.2.1` tag, so the secure example pulled a long-superseded image.
- Re-verified the v1.0 production gates on the v1.0.1 build (format, clippy, full
  test suite, backup/restore and upgrade gates, `cargo audit` / `cargo deny`,
  release-artifact verifier self-test, Docker image and Compose smokes, and
  connector conformance). No engine behavior changes were required.

## [1.0.0] - 2026-06-08

**Single-node production release, multi-node HA candidate preview.** AuraDB v1.0.0
supports production single-node deployments when configured with auth, TLS, backups,
monitoring, and the documented runbooks; single-node mode is the recommended
production mode. Multi-node static clustering remains an HA candidate preview — not
production HA, no production automatic failover, no production cluster readiness.
v1.0.0 carries forward all v0.9.2 behavior and adds no new database or cluster
architecture. **Aura Wire Protocol 1 is frozen for v1** (AWP 1 is the stable v1 wire
protocol, preserved across v1.x unless a security or correctness issue requires a
documented break), and **storage format v2 is frozen for v1** (the stable v1
single-node storage format, preserved across v1.x unless a safety, corruption, or
security issue requires a documented migration). Aura Connector v0.4.1 (and
compatible 0.4.x) is the supported client. Known limitations are unchanged: exact
vector search (not ANN/HNSW), tokenized full-text with term-frequency ranking (not
BM25 or hybrid fusion), single-node snapshot isolation (not serializable), and
multi-node as an HA candidate preview (no linearizable/follower reads, distributed
transactions, dynamic membership, sharding, multi-region, or Kubernetes operator).
See [docs/SUPPORT_POLICY.md](docs/SUPPORT_POLICY.md),
[docs/V1_0_RELEASE_NOTES.md](docs/V1_0_RELEASE_NOTES.md),
[docs/HA_RELEASE_CANDIDATE.md](docs/HA_RELEASE_CANDIDATE.md), and
[docs/V1_0_DECISION_CHECKLIST.md](docs/V1_0_DECISION_CHECKLIST.md).

### Added
- v1.0 support policy ([docs/SUPPORT_POLICY.md](docs/SUPPORT_POLICY.md)): what is
  supported, in preview, and unsupported, plus the security and upgrade support
  policy.
- Single-node production support statement (scoped to auth + TLS + backups +
  monitoring + runbooks).
- Aura Wire Protocol 1 compatibility statement (frozen for v1).
- Storage format v2 compatibility statement (frozen for v1).
- Upgrade guarantee statement ([docs/UPGRADING.md](docs/UPGRADING.md)): in-place
  upgrade from documented v0.x release fixtures; backup-first and `auradb check`
  before and after; no downgrade guarantee; rollback via restore from backup.
- Backup and restore release gate
  ([docs/PRODUCTION_READINESS.md](docs/PRODUCTION_READINESS.md),
  [docs/RELEASE.md](docs/RELEASE.md)).
- Security hardening review for the production single-node mode
  ([docs/SECURITY.md](docs/SECURITY.md)).
- v1.0 support matrix ([docs/COMPATIBILITY.md](docs/COMPATIBILITY.md),
  [README.md](README.md)).
- Production single-node runbook checklist
  ([docs/RUNBOOKS.md](docs/RUNBOOKS.md),
  [docs/PRODUCTION_READINESS.md](docs/PRODUCTION_READINESS.md)).
- v1.0 release notes ([docs/V1_0_RELEASE_NOTES.md](docs/V1_0_RELEASE_NOTES.md)).

### Changed
- Clarified multi-node as an HA candidate preview, not production HA, across the
  README and docs.
- Improved production-readiness and support documentation.
- Tightened release artifact verification
  ([scripts/verify_release_artifacts.sh](scripts/verify_release_artifacts.sh)): the
  `--tag` release-body check now requires the single-node production statement, the
  multi-node preview disclaimer, the AWP 1 statement, the storage format v2
  statement, and known limitations.
- Updated GitHub Actions for Node 24 compatibility: upgraded the `docker/*` actions
  (`setup-buildx-action` v3→v4, `build-push-action` v6→v7, `login-action` v3→v4,
  `metadata-action` v5→v6, `setup-qemu-action` v3→v4), resolving the v0.9.2 Node 20
  warning with no change to the Docker publish security posture.
- Refreshed the benchmark baseline (`benches/baseline/v1.0.0.json`); benchmark
  numbers remain machine-specific and warn-only.

### Fixed
- Resolved the non-blocking GitHub Actions Node 20 deprecation warning on the Docker
  build/publish actions.

## [0.9.2] - 2026-06-08

**Final HA candidate stabilization.** v0.9.2 is the last planned stabilization
patch for the HA release candidate before deciding what AuraDB v1.0.0 can honestly
claim. It finalizes the HA candidate evidence and gap list, adds a v1.0 decision
checklist, strengthens the leader-hint / client-address tests and runbooks after
`advertise_client_addr`, sharpens the HA smoke diagnostics and the published-image
post-release checklist, and maps the snapshot/compaction/old-leader-rejoin
coverage. It introduces **no** new cluster architecture and changes **no** Raft,
storage, query, MVCC, replication, or snapshot semantics except where a documented
bug is fixed. The storage format (v2) and the Aura Wire Protocol (AWP 1) are
unchanged, and Aura Connector v0.4.1 compatibility is preserved. Multi-node mode
remains a controlled static-cluster preview — **not** production HA — and
**single-node mode remains the recommended production mode.** See
[docs/HA_RELEASE_CANDIDATE.md](docs/HA_RELEASE_CANDIDATE.md),
[docs/V1_0_DECISION_CHECKLIST.md](docs/V1_0_DECISION_CHECKLIST.md), and
[docs/V0_9_2_RELEASE_NOTES.md](docs/V0_9_2_RELEASE_NOTES.md).

### Added
- Final HA candidate evidence and gap checklist
  ([docs/V1_0_DECISION_CHECKLIST.md](docs/V1_0_DECISION_CHECKLIST.md)): what v1.0
  can and cannot claim today, the requirements for single-node production and for
  production HA, the evidence that exists, the evidence still missing, and the
  recommended v1.0 scope.
- Additional leader-hint / client-address tests:
  `not_leader_uses_advertised_client_addr_after_multiple_re_elections` and
  `not_leader_hint_survives_old_leader_rejoin` (in `multi_node.rs`), and
  `docker_compose_docs_explain_in_network_vs_host_client_addr` (a docs-consistency
  test).
- A mapping of the snapshot/compaction/old-leader-rejoin scenarios to the existing
  v0.9.x tests that cover them (in
  [docs/HA_RELEASE_CANDIDATE.md](docs/HA_RELEASE_CANDIDATE.md) and
  [docs/TESTING.md](docs/TESTING.md)), so the coverage is auditable without
  duplicate tests.

### Changed
- Improved HA candidate release criteria documentation and the strict
  production-HA criteria, cross-linked to the v1.0 decision checklist.
- Improved the published-image post-release smoke checklist
  (`scripts/smoke_ha_candidate.sh`, `scripts/smoke_cluster_compose.sh`): image
  digest, per-node server versions, leader before/after kill, leader
  client-address source (advertised / status / fallback / probe), connector
  version, explicit pass/fail criteria, log preservation on failure, and
  `KEEP_ARTIFACTS=1`.
- Improved cluster troubleshooting and operator runbook guidance after a leader
  change (missing/unreachable leader hint, Docker in-network vs. host-published
  addresses, rotating `advertise_client_addr`, routing vs. no-leader, and
  collecting evidence for a v1.0 readiness report).

### Fixed
- Any v0.9.1 release, CI, HA-smoke, leader-hint, snapshot, compaction, or
  documentation issues found during validation. No code-behavior regression was
  found in v0.9.1; v0.9.2 is a proactive final stabilization patch (see
  [docs/V0_9_2_RELEASE_NOTES.md](docs/V0_9_2_RELEASE_NOTES.md)).

## [0.9.1] - 2026-06-08

**HA release-candidate stabilization** of the v0.9.0 candidate. v0.9.1 polishes
leader-hint propagation, strengthens leader-hint documentation and tests,
improves HA smoke reliability and diagnostics, adds snapshot/compaction coverage
across a leader change, and clarifies operator runbooks. It introduces **no** new
cluster architecture and changes **no** Raft, storage, query, MVCC, replication,
or snapshot semantics except where a documented bug is fixed. The storage format
(v2) and the Aura Wire Protocol (AWP 1) are unchanged, and Aura Connector v0.4.1
compatibility is preserved. Multi-node mode remains a controlled static-cluster
preview — **not** production HA — and **single-node mode remains the recommended
production mode.** See [docs/HA_RELEASE_CANDIDATE.md](docs/HA_RELEASE_CANDIDATE.md)
and [docs/V0_9_1_RELEASE_NOTES.md](docs/V0_9_1_RELEASE_NOTES.md).

### Added
- Optional `[cluster] advertise_client_addr`: this node's own client-facing
  address, reported as the leader client address (in `not_leader` hints and
  cluster status/health) while this node is the leader — closing the gap where a
  node could not name its own client address. Operator-declared and honest:
  never guessed, never a peer transport address, omitted when unset.
- Leader-hint propagation tests: `not_leader_includes_leader_client_addr_after_re_election`,
  `not_leader_hint_does_not_use_peer_addr_as_client_addr`,
  `not_leader_hint_omits_unknown_client_addr_safely`,
  `cluster_status_leader_client_addr_matches_not_leader_hint`,
  `leader_reports_its_own_client_addr_in_health`, and
  `docker_compose_cluster_not_leader_hint_has_client_addr_if_configured`.
- Snapshot/compaction-across-leader-change tests:
  `snapshot_install_after_leader_change`,
  `old_leader_rejoins_then_receives_snapshot_if_needed`,
  `snapshot_metrics_after_leader_change`, `compaction_after_leader_change`, and
  `snapshot_failure_after_leader_change_safe_to_retry`.
- HA smoke diagnostics: old/new leader and candidate addresses, the
  `leader_client_addr` hint at each leader, and the resolution path (direct hint
  vs. re-resolve fallback) in `run_connector_leader_change.py`.
- Operator runbook guidance for the `not_leader` leader-hint fallback and
  leader re-resolution.

### Changed
- Example and Docker Compose cluster configs declare `advertise_client_addr` and
  peer `client_addr`, with the Compose caveat documented (the in-network hint is
  not the host-published port, so host clients re-resolve — the documented
  fallback).
- Improved leader-hint documentation and conformance reporting.
- Improved HA smoke reliability and failure output.
- Improved cluster troubleshooting and runbook guidance.

### Fixed
- The leader could not report its own client address (a node is absent from its
  own peer list), so a `not_leader` hint or cluster status taken from the leader
  itself omitted `leader_client_addr`; `advertise_client_addr` now provides it.

## [0.9.0] - 2026-06-07

**HA release candidate** for the controlled static-cluster preview, not a
production HA guarantee. v0.9.0 strengthens failure testing, cluster
diagnostics, snapshot/compaction coverage, connector behavior under leader
change, operator recovery runbooks, the cluster backup/restore story, and the
release criteria. It introduces **no** new cluster architecture and changes
**no** Raft, storage, query, MVCC, replication, or snapshot semantics except
where a documented bug is fixed. The storage format (v2) and the Aura Wire
Protocol (AWP 1) are unchanged, and Aura Connector v0.4.1 compatibility is
preserved. Multi-node mode remains a controlled static-cluster preview — **not**
production HA — and **single-node mode remains the recommended production mode.**
See [docs/HA_RELEASE_CANDIDATE.md](docs/HA_RELEASE_CANDIDATE.md) and
[docs/V0_9_RELEASE_NOTES.md](docs/V0_9_RELEASE_NOTES.md).

### Added
- HA release-candidate criteria for the controlled static-cluster preview
  (`docs/HA_RELEASE_CANDIDATE.md`): support levels, required operator
  assumptions, the validated failure matrix, what is not yet production HA, and
  the strict criteria required before any future production HA claim.
- Cluster failure matrix and validation coverage, mapped across
  `docs/HA_RELEASE_CANDIDATE.md`, `docs/CLUSTER_TROUBLESHOOTING.md`, and
  `docs/TESTING.md`.
- Longer repeated fail-stop tests: `ha_repeated_leader_restart_3_cycles`
  (CI-safe), `ha_old_leader_rejoins_each_cycle`,
  `ha_repeated_restart_no_duplicate_apply`,
  `ha_repeated_restart_indices_converge`, and an `#[ignore]`d
  `ha_repeated_leader_restart_10_cycles_ignored` stress run.
- Larger snapshot install and compaction tests:
  `ha_snapshot_install_after_compaction_with_offline_follower`,
  `ha_snapshot_install_then_more_writes_converges`,
  `ha_snapshot_install_preserves_indexed_workload`,
  `ha_compaction_with_all_followers_caught_up`,
  `ha_compaction_with_offline_follower_requires_snapshot`,
  `ha_snapshot_failure_safe_to_retry`, `ha_snapshot_metrics_after_install`, and
  an `#[ignore]`d `ha_snapshot_large_ignored_stress`.
- Published-image HA smoke workflow and `scripts/smoke_ha_candidate.sh`
  (leader kill → new leader → old-leader rejoin → catch-up → status), wired as a
  manual / post-release job in `.github/workflows/cluster.yml`.
- Connector redirect-under-leader-change validation:
  `tests/conformance/python/run_connector_leader_change.py`.
- Operator recovery runbooks in `docs/RUNBOOKS.md` and
  `docs/CLUSTER_TROUBLESHOOTING.md`.
- Cluster backup and restore guidance and tests
  (`cluster_backup_before_and_after_leader_change`,
  `cluster_backup_restore_latest_leader_state`,
  `cluster_restore_live_cluster_rejected_or_documented`,
  `cluster_restore_to_single_node_then_bootstrap_preview_cluster`).
- GitHub Actions Node 24 maintenance.

### Changed
- Improved cluster recovery diagnostics and release criteria.
- Improved cluster preview testing and release checklist.
- Updated GitHub Actions versions to avoid Node 20 deprecation
  (`actions/setup-python` → v6, `actions/upload-artifact` /
  `actions/download-artifact` → v5; `actions/checkout`, `actions/cache`, and the
  `docker/*` actions were already on Node-24 majors).

### Fixed
- Any leader-change, snapshot-install, compaction, connector-redirect, workflow,
  or recovery-diagnostics bugs found during validation. No product behavior
  regressions were found in v0.8.1; this release is primarily preview hardening,
  validation, and documentation.

## [0.8.1] - 2026-06-07

Production-readiness **stabilization patch** for the v0.8.0 candidate. It narrows
in on the operational edges of v0.8.0 — backup/restore corner cases, resource-limit
boundaries, soak ergonomics, release-artifact verification, and runbook clarity —
without adding product features or changing semantics.

This release introduces **no** new database architecture and changes **no** Raft,
storage, query, MVCC, replication, or snapshot semantics. The storage format
(v2) and the Aura Wire Protocol (AWP 1) are unchanged, and Aura Connector v0.4.1
compatibility is preserved. Multi-node mode remains an experimental, opt-in
preview — **not** production HA — and **single-node mode remains the recommended
production mode.** All v0.8.0 behavior is preserved except where a documented bug
was fixed.

### Added
- Additional backup and restore edge-case coverage: empty database, schema-only
  export, a large single record, Unicode/escaped strings, deeply nested
  documents, vectors, relationship delete policies, full-text with punctuation,
  and document-path indexes after restore.
- Backup-verify rejection coverage: malformed JSONL, records for an undeclared
  collection, duplicate primary keys, truncated files, invalid schema sections,
  the per-line size bound, and a check that the verify report never echoes record
  contents.
- `auradb backup verify` now rejects a backup that carries two records with the
  same primary key — a corrupt or hand-edited dump whose restore would silently
  collapse two logical records into one. Only the collection and a count are
  reported; the key value is never printed.
- Additional resource-limit edge-case coverage: exact-boundary acceptance,
  one-past-boundary rejection, zero/absurd config validation, error-shape
  stability, payload redaction, the no-partial-commit guarantee on a refused
  staged write, and a structured single-message snapshot-size error.
- Release-artifact verification gained a SHA256SUMS-completeness check (no stray,
  unlisted asset), a `--tag` release-body honesty-wording check, and a
  network-free `--self-test` mode covering missing-archive, bad-checksum,
  wrong-version-name, and good-directory scenarios.
- Soak scripts now print timestamps, the binary version, and the data/log
  directories; honor `KEEP_ARTIFACTS=1`; allow a `LEADER_ADDR` override for the
  cluster preview; and emit a final machine-readable summary line.

### Changed
- The document-depth limit error now names the offending top-level field so an
  operator can find the over-nested path without bisecting the record. The field
  name is structural, not record content, so this leaks nothing.
- Improved production-readiness documentation and operator runbooks.
- Improved release checklist clarity.
- Improved backup/restore and resource-limit diagnostics.

### Fixed
- Resolved v0.8.0 documentation and release-tooling rough edges found during
  release validation (release-body wording verification, stray-asset detection,
  soak cleanup and artifact retention). No product behavior regressions were
  found in v0.8.0; this patch is primarily proactive stabilization.

## [0.8.0] - 2026-06-07

Production-readiness candidate for single-node deployments and a stronger cluster
**preview**. This release moves AuraDB from "impressive preview" toward "credible
early production candidate" for **single-node** mode — without overclaiming. It
focuses on hardening, validation, and operability rather than new product
features: a single-node production-readiness checklist, storage corruption drills
and a structured `auradb check --json`, backup/restore and upgrade drills, real
defensive resource limits, large-dataset and soak harnesses, performance
regression thresholds, a security hardening review, stronger cluster-preview
recovery tests, operator runbooks, and release-artifact reproducibility checks.

This release introduces **no** large new database features and changes **no**
Raft, storage, query, MVCC, replication, or snapshot semantics except where a real
bug was fixed during hardening. The storage format and the Aura Wire Protocol
(AWP v1) are unchanged, and Aura Connector v0.4.1 compatibility is preserved. It
is **not** production HA — there is no production automatic failover, no
linearizable follower reads, no distributed transactions, no dynamic membership,
and no sharding or multi-region. Multi-node mode remains an experimental, opt-in
preview; **single-node mode remains the recommended production mode.** All v0.7.1
behavior is preserved.

### Added
- Single-node production-readiness checklist (`docs/PRODUCTION_READINESS.md`).
- Storage corruption drills and additional consistency checks, surfaced through a
  structured `auradb check --json` report (storage, catalog, indexes, planner
  stats, raft, snapshots, warnings, errors).
- Backup and restore drill coverage (mixed dataset, indexes/stats, relationships,
  vectors, post-compaction, JSONL corruption rejection) and `auradb backup verify`.
- Upgrade drill coverage across real release fixtures.
- Resource limit validation and configurable defensive bounds (query
  limit/offset, vector dimension, full-text query tokens, document nesting depth,
  transaction write-set size, backup input line size).
- Large dataset validation (CI-safe smokes plus ignored stress).
- Soak and repeatability harness (`scripts/soak_single_node.sh`,
  `scripts/soak_cluster_preview.sh`).
- Performance regression threshold tooling (`scripts/compare_benchmarks.py`,
  `auradb bench compare --fail-threshold-percent`) and a v0.8.0 baseline.
- Security hardening review documentation and redaction tests.
- Cluster preview recovery hardening (repeated restart, snapshot, reconnect,
  doctor smokes).
- Operator runbooks (`docs/RUNBOOKS.md`).
- Release artifact reproducibility checks (`scripts/verify_release_artifacts.sh`).
- `docs/V0_8_RELEASE_NOTES.md`.

### Changed
- Improved production-readiness documentation and the release checklist.
- Improved operational guidance for backup, restore, upgrade, and recovery.
- Improved CI coverage for recovery and artifact validation.

### Fixed
- Storage, backup/restore, upgrade, resource-limit, recovery, or diagnostics bugs
  found during validation.

## [0.7.1] - 2026-06-06

Connector ergonomics polish. This release coordinates with Aura Connector v0.4.1
to improve the developer experience around the controlled multi-node **preview**:
clearer cluster-preview docs for Python connector users, hardened connector
cluster conformance guidance, and additional leader-hint and safe-redirect
examples.

This release adds **no** new database architecture and changes **no** Raft,
storage, query, MVCC, replication, or snapshot semantics. The `not_leader` payload
is byte-for-byte the same as v0.7.0, the storage format is unchanged, and the Aura
Wire Protocol (AWP v1) is unchanged. It is **not** production HA — there is no
production automatic failover, no linearizable follower reads, no distributed
transactions, no dynamic membership, and no sharding or multi-region. Multi-node
mode remains an experimental, opt-in preview; **single-node mode remains the
recommended production mode.** All v0.7.0 behavior is preserved.

### Added
- Aura Connector v0.4.1 compatibility notes across the cluster and conformance
  docs (`docs/AURA_CONNECTOR_COMPATIBILITY.md`, `docs/COMPATIBILITY.md`,
  `docs/CONFORMANCE.md`).
- Stronger connector cluster conformance guidance for the published connector,
  including how PR vs. release/tag runs select a local or published connector.
- Additional examples for leader hints (`auradb cluster leader`) and safe
  redirects in `examples/cluster/`.
- `docs/V0_7_1_RELEASE_NOTES.md`.

### Changed
- Improved cluster-preview docs for Python connector users.
- Improved the release checklist for connector-first coordinated releases
  (`docs/RELEASE.md`).

### Fixed
- Documentation and conformance-guidance bugs found during coordinated validation.
  No server behavior changed.

## [0.7.0] - 2026-06-06

Connector cluster ergonomics. This release coordinates with Aura Connector
v0.4.0 to give Python clients a clean, safe, cluster-aware experience for the
controlled multi-node **preview**. The `not_leader` response now carries a
stable, structured payload — the leader's client address, the leader and current
node ids, term, role, and a usable `leader_hint` — alongside the existing human
message, so a connector can redirect to the leader without parsing text.

This is **not** production HA. There is no production automatic failover claim,
no linearizable follower reads, no distributed transactions, no dynamic
membership, and no sharding or multi-region. Multi-node mode remains an
experimental, opt-in preview; **single-node mode remains the recommended
production mode.** All v0.6.2 behavior, the storage format, and the Aura Wire
Protocol (AWP v1) are preserved — the new `not_leader` fields are purely additive
and older clients ignore them.

### Added
- Stable, structured `not_leader` payload: an additive `not_leader` object on the
  error frame carrying `current_node_id`, `leader_node_id`, `leader_client_addr`,
  `leader_hint`, `term`, and `role`, built from the node's current cluster view.
  Fields are present only when genuinely known and never carry secrets.
- Connector cluster conformance runner
  (`tests/conformance/python/run_connector_cluster.py`) covering leader writes,
  follower `not_leader`, the reconnect helper, the bounded redirect helper, and
  transaction-safe redirect rejection against Aura Connector v0.4.x.
- A Docker-cluster Python connector example (`examples/cluster/python_connector.py`).
- Cluster-aware documentation for Aura Connector v0.4.x.

### Changed
- Improved client-facing cluster-preview error ergonomics: the `not_leader` error
  is enriched with machine-readable leader-routing hints at dispatch.
- Updated the Aura Connector compatibility matrix for the cluster ergonomics
  (Aura Connector 0.4.x ↔ AuraDB 0.7.x).

## [0.6.2] - 2026-06-06

Repeated chaos and larger-state recovery hardening. This patch release makes the
controlled multi-node **preview** more reliable under repeated failures, larger
data sets, and recovery-heavy scenarios: repeated leader restart / re-election
cycles, larger multi-model data-set recovery, multi-model snapshot install, a
peer reconnect storm, deterministic network-interruption (partition/heal)
simulations, and recovery-focused diagnostics.

This is **not** production HA. There is no production automatic failover claim,
no linearizable follower reads, no distributed transactions, no dynamic
membership, and no sharding or multi-region. Multi-node mode remains an
experimental, opt-in preview; **single-node mode remains the recommended
production mode.** All v0.6.1 behavior, the storage format, the Aura Wire
Protocol (AWP v1), and Aura Connector 0.3.x compatibility are preserved — no
connector release is required.

### Added
- Repeated leader restart and re-election testing: a required two-cycle
  kill-leader/elect/restart test that asserts every node reconverges on the
  identical committed record set with no duplicate apply and an incrementing
  leader-change metric, plus an `#[ignore]`d five-cycle stress variant.
- Larger multi-model data-set recovery validation: a follower is stopped, the
  majority commits a larger run of records spanning scalar, secondary-indexed,
  full-text, document-path, and vector fields, and after restart the follower's
  counts, spot reads, full-text search, document-path queries, vector
  nearest-neighbor results, and planner-used indexes are verified to match the
  rest of the cluster — with an `#[ignore]`d 5,000-record stress variant and a
  full-cluster-restart check.
- Multi-model snapshot install: snapshot-install catch-up that preserves
  full-text, document-path, and vector records (the live majority compacts past
  the entries the follower needs, forcing a snapshot install).
- Peer reconnect storm testing: a follower is disconnected and reconnected
  repeatedly while the majority keeps committing; replication recovers each time
  with no duplicate apply and the follower holds a live peer connection after the
  storm.
- Deterministic network-interruption (partition/heal) simulations using an
  in-process transport partition control: a majority partition continues
  operating, a leader partitioned into a minority cannot commit, a healed
  partition repairs a follower's log, and a partitioned leader triggers a
  re-election and reconverges on heal. The isolated node keeps running (its
  in-memory state is preserved), unlike a stop/restart.
- Recovery diagnostics: `auradb cluster status --addr` now reports
  `leader_changes` (a cumulative leadership-instability signal), and
  `auradb cluster doctor --addr` warns on a peer reconnect storm (a peer still
  disconnected after many connection attempts) and on repeated leader changes.
- Published-image cluster smoke retained and strengthened as a release gate: the
  release checklist requires inspecting the GHCR multi-arch manifest and running
  the published-image compose smoke before a release is considered done.

### Changed
- Improved cluster preview recovery test coverage (repeated fail-stop,
  larger-state, multi-model snapshot install, reconnect storm, partition/heal).
- Improved troubleshooting guidance for repeated fail-stop, reconnect-storm, and
  lagging-peer scenarios.
- The cross-process preview test harness gained a transport partition control
  (`drop_peer_link` / `heal_peer_link`) used only by tests and chaos scenarios;
  it has no configuration or CLI surface.

### Fixed
- A multi-model snapshot-install test could read the snapshot-install counter a
  hair before it was incremented (the follower's engine reaches the target record
  count inside the install handler first); it now polls the counter with a bounded
  deadline.

## [0.6.1] - 2026-06-06

Snapshot install and published-cluster smoke hardening. This patch release makes
the v0.6.0 controlled multi-node preview more reliable, observable, and
repeatable: multi-architecture Docker images, larger and concurrent-write
snapshot-install validation, snapshot-needed and follower-lag diagnostics,
cluster backup/restore dry-run planning, and a published-image cluster smoke
checklist.

This is **not** production HA. There is no production automatic failover claim,
no linearizable follower reads, no distributed transactions, no dynamic
membership, and no sharding or multi-region. Multi-node mode remains an
experimental, opt-in preview; **single-node mode remains the recommended
production mode.** All v0.6.0 behavior, the storage format, the Aura Wire
Protocol (AWP v1), and Aura Connector 0.3.x compatibility are preserved.

### Added
- Multi-architecture Docker image publishing: the tag workflow builds and pushes
  a `linux/amd64` + `linux/arm64` manifest to `ghcr.io/ohswedd/auradb:0.6.1` and
  `:latest` via Docker Buildx (arm64 built under QEMU in CI). PR/branch builds
  build `linux/amd64` through buildx without publishing. On Apple Silicon,
  `docker pull` selects the arm64 variant automatically.
- Larger snapshot-install validation: a CI-safe larger run plus `#[ignore]`d
  1,000-entry and 10,000-entry stress scenarios, asserting data, index, planner,
  and MVCC-timestamp convergence after a snapshot install.
- Snapshot install under concurrent leader writes: the leader keeps committing
  while a follower installs a snapshot and resumes AppendEntries, with no
  duplicate apply and full convergence.
- Snapshot-needed and follower-lag diagnostics: per-peer `lag_entries`,
  `needs_snapshot`, `snapshot_in_progress`, and a `catch_up_state`
  (`normal` / `probing` / `snapshot_needed` / `snapshot_installing` /
  `caught_up` / `unknown`), plus cluster-level snapshot diagnostics (last
  installed boundary, last install time, last error, bytes sent/installed,
  in-progress gauge), surfaced by `auradb cluster status --addr` and a new live
  `auradb cluster doctor --addr`.
- Additional snapshot-install metrics: `auradb_cluster_snapshot_needed_total`,
  `auradb_cluster_snapshot_bytes_sent_total`,
  `auradb_cluster_snapshot_bytes_installed_total`,
  `auradb_cluster_snapshot_in_progress`, and `auradb_cluster_snapshot_last_error`.
- Cluster backup and restore dry-run tooling: `auradb cluster backup-plan`
  inspects a data dir and reports what a logical backup would include, exclude,
  where it restores, and which secrets are referenced (redacted);
  `auradb cluster restore-plan` inspects a JSONL backup and reports what a
  restore would load. Neither writes data.
- Published GHCR cluster smoke release checklist and an enhanced
  `scripts/smoke_cluster_compose.sh` that prints the image, node ports, leader,
  quorum, peer states, and teardown result; the manual `published-image-smoke`
  workflow inspects the multi-arch manifest before running the smoke.
- Connector leader-hint UX review (docs): Aura Connector 0.3.x remains compatible
  but is not cluster-routing-aware; manual leader routing is documented, with
  tests pinning the `not_leader` leader-hint message and no-infinite-retry
  contract.

### Changed
- Improved the Docker publish workflow (multi-arch) and the cluster smoke
  documentation and release checklist.
- Improved cluster troubleshooting for followers that need a snapshot and for
  lagging followers.
- Improved observability for snapshot install and follower lag.

### Fixed
- Snapshot-install, follower-lag, Docker publish, cluster-smoke, and diagnostics
  issues found during validation are addressed; no behavioral regressions to
  single-node or single-node-cluster modes.

## [0.6.0] - 2026-06-06

Cluster ergonomics and fail-stop recovery preview. This release improves the
controlled multi-node preview experience and validates fail-stop recovery
behavior: a leader is stopped, the surviving majority elects a new leader, the
new leader accepts writes, and the old node rejoins as a follower and catches
up. It also adds the first real **peer snapshot install over the wire** (a
bounded, single-message transfer for the preview), sharper fail-stop
diagnostics, and operator runbooks for peer certificate/token rotation and
cluster backup/restore.

This is **not** production HA. There is no production automatic failover claim,
no linearizable follower reads, no distributed transactions, no dynamic
membership, and no sharding or multi-region. Multi-node mode remains an
experimental, opt-in preview; **single-node mode remains the recommended
production mode.** All v0.5.x single-node and single-node-cluster behavior, the
storage format, the Aura Wire Protocol, and Aura Connector 0.3.x compatibility
are preserved.

### Added
- Published-image Docker Compose cluster smoke workflow: `scripts/smoke_cluster_compose.sh`
  now honors `AURADB_IMAGE` so the same three-node Compose smoke can run against
  a locally built image (`auradb:0.6.0`) or a published image
  (`ghcr.io/ohswedd/auradb:0.6.0`). A manual `published-image-smoke` CI job
  verifies the published image post-release.
- Leader kill and automatic re-election preview tests: a stopped leader's term
  is taken over by the surviving majority, the new leader accepts writes, the
  old leader restarts as a follower and catches up, all nodes converge, and the
  leader-change metric increments.
- Connector write-recovery validation against a newly elected leader: after a
  leader kill, the `not_leader` response carries a leader hint and a retryable
  flag, and a client that retries against the new leader's address succeeds.
- Larger follower restart and catch-up tests across indexed, document-path,
  vector, and transaction-batch workloads, asserting no duplicate application
  and preserved MVCC timestamps and indexes after restart.
- Peer snapshot install over the wire (`InstallSnapshotRequest` /
  `InstallSnapshotResponse`): when a follower needs entries the leader has
  already compacted away, the leader sends a bounded, single-message snapshot;
  the follower validates cluster id, snapshot format, last-included index/term,
  digest, storage format, and size limit, restores atomically into a staging
  area, advances its Raft compaction boundary, and resumes AppendEntries.
  Oversized, wrong-cluster, bad-digest, and future-format snapshots are
  rejected, and a failed install preserves existing follower state.
- Cluster backup and restore runbook: leader-side logical backup
  (`auradb dump`) exports the latest committed state; `auradb restore`
  rebuilds a single-node data directory that can seed a fresh preview cluster.
- Peer certificate and token rotation runbook with rolling-restart procedures,
  CA-rotation guidance, and SAN/CA/token mismatch diagnosis.
- Improved fail-stop recovery diagnostics: cluster health now reports leader
  changes, last leader-change time and reason, per-peer last disconnect reason,
  last successful append time, snapshot install status, and `not_leader`
  rejected-write counts; surfaced via `auradb cluster diagnostics --addr` and
  `auradb cluster events --addr`.
- Replicated-cluster benchmark baseline: `benches/baseline/v0.6.0.json`.

### Changed
- Improved multi-node preview documentation, diagnostics, and operator
  workflows across the clustering, Raft, replication, operations, security,
  observability, and troubleshooting docs.
- Improved cluster readiness and leader-wait behavior and diagnostics output,
  with explicit preview and public-cluster warnings retained.

### Fixed
- Fail-stop recovery, snapshot install, connector routing, peer transport, and
  catch-up issues found during validation are addressed; no behavioral
  regressions to single-node or single-node-cluster modes.

## [0.5.2] - 2026-06-05

Multi-node preview hardening, follow-up fix. A patch release that fixes the
development certificates generated for the multi-node preview so the peer
(cluster) transport's **mutual TLS** actually works. No format or wire change;
single-node mode remains the recommended production mode.

### Fixed
- `auradb cert generate-dev` certificates were issued with a server-only Extended
  Key Usage, so a node presenting its certificate as a *client* certificate when
  dialing a peer was rejected by the peer's client-cert verifier
  ("certificate does not allow extended key usage for client authentication") and
  the multi-node TLS cluster (e.g. the Docker Compose preview) never formed a
  quorum. Generated certificates now allow **both server and client
  authentication**, which the peer transport's mutual TLS requires. Client-facing
  server TLS is unaffected. This regressed only the v0.5.1 generated-certificate
  Docker cluster path; loopback (plaintext) preview clusters were never affected.

### Changed
- The peer dialer now logs a failed connect/handshake at debug level instead of
  silently swallowing the error, so peer TLS and handshake failures are
  diagnosable.

### Added
- A regression test that forms a real two-node TLS cluster using the actual
  `auradb cert generate-dev` output, so a server-only-EKU regression is caught.

## [0.5.1] - 2026-06-05

Multi-node preview hardening. A patch release that makes the v0.5.0 controlled
multi-node preview safer, easier to operate, and more trustworthy. It adds
local Docker cluster automation, sharper cluster diagnostics, more honest
`not_leader` ergonomics, and additional leader-restart and follower-catch-up
coverage. No production-clustering claims are made: multi-node mode remains an
experimental, opt-in preview, and single-node mode remains the recommended
production mode. All v0.5.0 behavior, the storage format, the Aura Wire
Protocol, and Aura Connector 0.3.x compatibility are preserved.

### Added
- Development certificate generation for local multi-node Docker clusters:
  `auradb cert generate-dev` now accepts `--server-name` and repeatable `--san`
  flags to emit per-node certificates with explicit Subject Alternative Names,
  and `examples/cluster/generate-dev-certs.sh` drives it to produce a local CA
  and node1/node2/node3 certificates under a git-ignored `certs/` directory.
- Live Docker Compose cluster smoke validation (`scripts/smoke_cluster_compose.sh`):
  generates dev certs, starts the three-node Compose cluster, waits for a
  leader, reports status, and tears the cluster down.
- Leader restart and re-election smoke tests: a stopped leader's term is taken
  over by the surviving majority, the old leader rejoins as a follower and
  catches up, and all nodes converge.
- Larger follower catch-up tests: a follower that misses a long run of committed
  entries (including transaction batches and a compacted-log boundary) replays
  its durable log and is brought current by the leader.
- Peer TLS certificate rotation guidance and validation: documented rolling
  rotation plus tests that a wrong CA, a wrong SAN, and a peer-token mismatch are
  rejected, and that a node presenting a freshly rotated certificate is accepted.
- Better cluster failure diagnostics: `auradb cluster status --addr` now queries
  a running server for live role, leader, quorum, replication indices, and
  per-peer reachability; `auradb cluster doctor` explains no-leader, no-quorum,
  unreachable-peer, and public-cluster-without-TLS conditions.
- Replicated write latency baseline: `benches/baseline/v0.5.1.json` records the
  same-machine baseline used for regression tracking.
- Connector `not_leader` behavior validation: tests assert the leader hint, the
  retryable guidance, and that the same client connection stays usable after a
  `not_leader` response.

### Changed
- Improved multi-node preview deployment documentation across `docs/CLUSTERING.md`,
  `docs/SECURITY.md`, `docs/OPERATIONS.md`, and `docs/CLUSTER_TROUBLESHOOTING.md`.
- Improved cluster diagnostics and troubleshooting output, including explicit
  preview and public-cluster warnings.
- Improved preview guardrails and operator guidance for peer TLS and peer tokens.

### Fixed
- Peer transport, leader election, follower catch-up, Docker cluster, and
  diagnostics issues found during validation are addressed; no behavioral
  regressions to v0.5.0 single-node or single-node cluster modes.

## [0.5.0] - 2026-06-05

Controlled multi-node server preview. The first release in which AuraDB server
processes can form a real cross-process cluster, electing a leader and
replicating writes over a dedicated peer transport. This is an explicit,
experimental preview intended for local testing and early validation only;
single-node mode remains the recommended production path. Cross-process peer
networking is disabled by default and must be turned on with both
`[cluster] enabled = true` and `[cluster] experimental_multi_node = true`. All
v0.4.1 behavior, storage format, and the Aura Wire Protocol are preserved, and
Aura Connector 0.3.x remains compatible.

### Added
- Experimental cross-process peer networking: a dedicated, length-delimited,
  CRC32-checksummed peer transport with a versioned `PeerHello` handshake that
  verifies protocol version, cluster id, and node id, carries a shared peer
  authentication token, supports TLS, and returns structured `PeerError` and
  `Unsupported` responses (snapshot install is not implemented and is reported as
  unsupported rather than silently ignored).
- Static multi-node cluster preview: a fixed peer set declared in configuration
  (`[[cluster.peers]]` with `node_id` and `addr`). No join, leave, or dynamic
  membership.
- Secure peer transport baseline: loopback-only peer networking may run without
  TLS for local preview; any non-loopback peer address fails closed unless
  `allow_experimental_public_cluster = true`, which additionally requires TLS and
  a peer authentication token.
- Three-node local cluster example (`examples/cluster/`, `docker-compose.cluster.yml`)
  with per-node configs, persistent volumes, separate client and cluster ports,
  and health checks.
- Real server-process leader election over the peer transport, driven by a real
  clock in a background task.
- Real server-process replicated writes: the leader appends to its Raft log,
  replicates via AppendEntries, commits on majority, and followers apply
  committed entries.
- Follower catch-up after restart: a restarted follower replays its durable log
  and is brought current by the leader.
- Cluster status across peers: live `auradb cluster status|peers|leader` against a
  running server, including per-peer connection state, match/next index, and
  replication lag.
- Multi-node integration tests that spawn real server tasks bound to real TCP
  sockets with readiness checks and bounded polling (no fixed sleeps).
- Connector validation against the elected leader.
- Cluster troubleshooting improvements for the multi-node preview.
- New cluster CLI commands: `auradb cluster leader`, `auradb cluster wait-ready`,
  and `auradb cluster wait-leader` (with `--timeout-secs`, `--json`, and auth/TLS
  flags).
- New peer and Raft metrics (`auradb_peer_connected`,
  `auradb_peer_replication_lag_entries`, `auradb_raft_elections_total`,
  `auradb_raft_election_timeouts_total`, `auradb_raft_append_entries_failures_total`,
  `auradb_raft_heartbeat_latency_ms`, `auradb_cluster_quorum_available`).

### Changed
- Cluster mode can now run with static peers when explicitly enabled via
  `experimental_multi_node = true`; without that flag, a non-empty peer set still
  fails closed exactly as in v0.4.1.
- Cluster diagnostics include peer reachability and replication state, and the
  cluster doctor warns about preview mode, public-cluster mode, missing leader or
  quorum, lagging or unreachable peers, and unsupported snapshot install.

### Fixed
- Cluster networking, Raft, replication, catch-up, and status bugs found during
  multi-node validation.

## [0.4.1] - 2026-06-05

Raft durability and cluster-mode hardening. A patch release that strengthens the
Raft and replication groundwork from v0.4.0 before any real cross-process
multi-node preview. No storage-format or wire-protocol change: multi-node server
deployment remains experimental and disabled by default, and single-node mode
remains the recommended production path. All v0.4.0 behavior is preserved.

### Added
- Raft log compaction boundary validation: a compactable-prefix calculation that
  refuses to discard entries that are not safely applied or are beyond the
  committed index, preserves the last included index and term, persists
  `raft-compaction.json`, and surfaces a structured `Compacted` error for reads
  before the retained prefix. AppendEntries consistency checks understand the
  compacted prefix.
- Snapshot restore edge-case tests and a richer snapshot manifest (cluster id,
  node id, storage-format version, created-at timestamp), with restore that is
  atomic (build in a temporary directory, validate, then swap), refuses to
  overwrite a non-empty target without `--force`, and rejects future formats,
  cluster-id mismatch, corrupt manifests, and digest mismatches.
- Raft apply idempotency tests under restart and crash-like sequences (commit
  before apply, partial apply, apply before watermark update).
- Cluster metadata corruption tests (missing, malformed, future-format, and
  partial identity) that fail closed.
- Stronger peer configuration validation: duplicate peers, a peer equal to the
  local node id, and any non-empty peers list are rejected with clear errors in
  this release (cross-process peers are not enabled).
- Single-node cluster overhead benchmarks (`benches/baseline/v0.4.1.json`,
  `auradb-cluster` `cluster_overhead` bench) comparing direct and single-node
  cluster write/read paths for same-machine regression tracking.
- Deterministic multi-node partition tests (minority cannot commit, majority
  elects a leader, old leader steps down on rejoin, committed entries survive a
  leader change, an uncommitted old-leader entry never commits and is repaired
  away).
- Cluster troubleshooting documentation
  ([docs/CLUSTER_TROUBLESHOOTING.md](docs/CLUSTER_TROUBLESHOOTING.md)).
- Cluster operational diagnostics: `auradb cluster compact-log [--dry-run]
  [--json]` and `auradb snapshot create|inspect|restore`.

### Changed
- Improved `auradb cluster status` / `auradb cluster doctor` output (JSON modes,
  clearer peer-rejection and durability warnings).
- Improved Raft durability checks around the compaction boundary and metadata.
- Improved cluster-mode documentation and release guardrails.

### Fixed
- Hardened fail-closed handling of corrupt cluster metadata, corrupt Raft
  compaction metadata, and inconsistent snapshot manifests found during
  validation.
- Gave each benchmark run a unique scratch directory (process id plus a per-call
  counter) so concurrent `auradb bench` runs in one process no longer race on a
  shared temporary path.

## [0.4.0] - 2026-06-05

The replication and Raft foundation for future clustered deployments. This
release introduces a correct, durable, testable cluster foundation. **Single-node
mode remains the recommended production path.** Multi-node clustering is
experimental: the Raft and replication core is validated by deterministic
in-process tests, but cross-process multi-node server deployment is not enabled
(configuring peers is rejected at startup). When cluster mode is disabled — the
default — all v0.3.1 behavior is preserved byte-for-byte.

### Added
- Stable node identity (`NodeId`) and cluster identity (`ClusterId`), persisted
  under `<data_dir>/cluster/` and created by `auradb init`.
- Cluster metadata and configuration (the `[cluster]` config table), validated
  at startup; unknown future metadata formats are rejected (fail closed).
- A durable, checksummed Raft log abstraction with corruption detection and
  crash-safe recovery (`auradb-raft`).
- A minimal, deterministic Raft state machine: follower/candidate/leader roles,
  elections, `RequestVote`, `AppendEntries`, heartbeats, log repair, and commit
  advancement, driven by a logical test clock.
- Single-node Raft mode: when cluster mode is enabled with no peers, every write
  is ordered through a durable local Raft log and replayed on restart.
- A leader-and-follower role model with a leader-only write path; followers
  reject writes with a structured `not_leader` error and a leader hint.
- A replicated command model and an idempotent replicated apply path; the MVCC
  commit timestamp is the Raft log index, so replicas derive identical ordering.
- A versioned snapshot boundary (`SnapshotManifest`) for future state transfer,
  with local create and restore.
- Cluster status and diagnostics: `auradb cluster init|status|peers|doctor|
  bootstrap`, plus cluster fields in `auradb status --json`, `auradb doctor`,
  and the server health report.
- Replication and Raft metrics (term, commit/applied/last-log index, leader
  changes, votes, AppendEntries counters, replication lag, apply errors, apply
  latency).
- Deterministic Raft and replication tests, including in-process multi-node
  consensus and replicated-apply tests, plus a single-node cluster example
  config (`examples/auradb.cluster.local.toml`).

### Changed
- The internal write path can be routed through the replication layer when
  cluster mode is enabled; the default (cluster-disabled) path is unchanged.
- Server health and status include an additive `cluster` section. The Aura Wire
  Protocol version is unchanged; Aura Connector 0.3.x remains fully compatible.

### Fixed
- Replication, recovery, and cluster-mode correctness issues found during
  validation (idempotent apply on restart; commit-order preservation through the
  Raft log; fail-closed handling of unknown future cluster, Raft, and snapshot
  formats).

## [0.3.1] - 2026-06-05

MVCC stabilization, upgrade confidence, and operational guardrails. A
stabilization release before replication and Raft work: it hardens the MVCC
transaction lifecycle so a long-lived or abandoned transaction can no longer pin
versions forever without visibility, adds transaction timeouts and an
abandoned-transaction reaper, strengthens GC validation, and surfaces MVCC
pressure through metrics, status, and `doctor` warnings. All v0.3.0 behavior is
preserved and Aura Connector 0.3.x remains compatible (no connector release is
required). This release still implements snapshot isolation, **not** serializable
isolation, and adds no clustering, replication, or Raft.

### Added

- Transaction timeout and abandoned transaction cleanup: an idle transaction past
  `[mvcc] transaction_timeout_secs` is reaped by the abandoned-transaction reaper,
  its snapshot released and further operations rejected with a structured
  `transaction_timeout` error.
- Active transaction registry tracking id, read timestamp, start time, last
  activity, connection id, and state; GC reclaims from this registry, never stale
  leaked state.
- MVCC pressure metrics: `auradb_mvcc_active_transactions`,
  `auradb_mvcc_oldest_snapshot_age_seconds`, `auradb_mvcc_retained_versions`,
  `auradb_mvcc_gc_runs_total`, `auradb_mvcc_gc_reclaimed_versions_total`,
  `auradb_mvcc_gc_reclaimed_bytes_total`, `auradb_mvcc_transaction_timeouts_total`,
  and `auradb_mvcc_conflicts_total`.
- Operational warnings in `auradb doctor` for long-lived snapshots, version
  pressure, disabled GC, disabled transaction timeouts, and stale statistics.
- Stronger MVCC garbage collection validation, plus `auradb gc --dry-run` and
  `auradb gc --json`, and `bytes_reclaimed` in the GC report.
- Additional upgrade safety tests across genuine v0.1.0, v0.2.0, v0.2.1, and
  v0.3.0 release fixtures into v0.3.1.
- Query planner regression tests and backup/restore-with-GC tests.
- Benchmark regression baseline comparison: `auradb bench compare --baseline … --current …`
  with an optional `--fail-threshold-percent` for CI.
- Improved `EXPLAIN ANALYZE` output: estimated-vs-actual rows, planner-stats
  version, selected-index reason, MVCC snapshot timestamp, and a stale-statistics
  warning (all additive JSON fields).

### Changed

- Improved cleanup behavior for dropped or disconnected transactions: a
  connection's transactions are rolled back on disconnect, and the reaper releases
  any that are abandoned.
- Health and `status` now report active snapshots and MVCC pressure (additive
  `mvcc` section in the health report).
- Improved documentation for snapshot isolation and version retention.

### Fixed

- An abandoned transaction handle (dropped without commit or rollback) no longer
  pins MVCC versions indefinitely: the abandoned-transaction reaper releases it.

## [0.3.0] - 2026-06-05

MVCC and query planner foundations. AuraDB now stores multiple committed
versions of each record and serves transactional reads from a snapshot pinned at
`begin`, giving **single-node snapshot isolation** with optimistic write-conflict
detection. Query reads route through a cost-based planner that uses persisted
statistics to choose an access path, and `EXPLAIN ANALYZE` reports measured
execution metrics. The on-disk storage format moves to v2; a v0.1.0/v0.2.x
directory is migrated transparently on first open. This release preserves all
v0.2.1 behavior for non-transactional reads and remains compatible with Aura
Connector 0.3.x (no connector release is required).

This release implements snapshot isolation, **not** serializable isolation.

### Added

- MVCC record versions: each record id maps to an ordered version chain, and a
  delete is a committed tombstone version. Versions, timestamps, and tombstones
  survive restart.
- Snapshot isolation with transaction read timestamps pinned at `begin`: a
  transaction sees committed state as of its begin-time snapshot (plus its own
  staged writes) and does not observe writes committed by other transactions
  after it began.
- Optimistic write-conflict detection (first-committer-wins): commit aborts with
  `Error::Conflict` when a record the transaction wrote was modified by a
  transaction that committed after the snapshot was pinned (covers write-write,
  update-delete, and delete-update conflicts).
- Version garbage collection (`auradb gc`, plus optional background GC): reclaims
  versions no active transaction can observe and drops fully-deleted records,
  always retaining the latest version and at least `min_retained_versions`.
- Query planner with costed index selection: a plan tree (point lookup, index
  lookup, document-path / full-text index lookup, vector search, scan, and the
  filter/sort/limit/offset/projection/relationship-include operators) chosen by
  estimated cost from row counts and per-field cardinality.
- Persisted planner statistics (`planner_stats.json`): row counts, field
  cardinality, vector counts, full-text document counts, and average record size,
  recomputed by `auradb stats analyze` and kept current on each mutation.
- `EXPLAIN ANALYZE` with execution metrics: scanned/matched/returned rows,
  execution and planning time, the index used, and the snapshot timestamp when
  run inside a transaction. Carried over the wire as an optional flag in the raw
  Query IR, so no protocol break.
- New CLI commands: `auradb gc`, `auradb stats analyze`, `auradb stats show`.
- New `[mvcc]` server configuration: `gc_enabled`, `gc_interval_secs`,
  `min_retained_versions`.
- MVCC, planner, and `EXPLAIN ANALYZE` benchmarks (`benches/mvcc.rs`,
  `benches/planner.rs`, `benches/explain_analyze.rs`) and a v0.3.0 baseline.
- Transaction isolation, planner, and `EXPLAIN ANALYZE` conformance scenarios.
- Upgrade tests from real v0.2.0 and v0.2.1 release fixtures to v0.3.0.

### Changed

- Transaction reads now use a stable begin-time snapshot instead of reading the
  latest committed state.
- Query execution now routes through the planner before execution.
- Index selection now considers statistics and estimated cost, choosing the most
  selective index among candidates and a full scan when no index applies.
- The on-disk storage format is now v2 (commit-timestamped version chains). A
  v1 (≤ 0.2.x) directory is migrated to v2 on first open; an unknown future
  format is still rejected.

### Fixed

- Transactional reads no longer observe writes committed by other transactions
  after the reading transaction began (previously they saw the latest committed
  state).

## [0.2.1] - 2026-06-05

Operational polish, safer defaults, release confidence, and deployment
readiness. This patch release preserves all v0.2.0 behavior; it adds deployment
examples, an operational token-rotation command, and durability and
compatibility coverage in CI.

### Added

- Secure Docker Compose example (`docker-compose.secure.yml`) that runs AuraDB
  with authentication and TLS enabled, a non-root user, a mounted config, a data
  volume, a mounted certificate directory, and a healthcheck. The token hash is
  supplied through an environment variable rather than committed in plaintext.
- Production configuration templates: `examples/auradb.secure.toml` (auth and
  TLS enabled, redacted token-hash placeholder) and `examples/auradb.local.toml`
  (loopback, auth and TLS disabled, development only), plus an
  `examples/production/` deployment bundle.
- Token rotation support: `auradb auth rotate-token` re-hashes a new token with
  Argon2id, writes the configuration atomically, preserves unrelated fields,
  optionally backs up the previous configuration, validates the result, and
  never writes a plaintext token.
- Backup and restore verification: an integration test that dumps a database
  containing scalar, document, vector, relationship, full-text, and
  document-path data and restores it into a fresh data directory, then verifies
  records, schema, indexes, and search.
- Upgrade coverage from an AuraDB v0.1.0 data directory: a committed fixture
  written by the v0.1.0 binary is opened by v0.2.1, validated, and its indexes
  rebuilt, with `auradb check` passing afterward.
- Chaos restart test that drives writes, updates, and deletes against the engine
  with deterministic crash-and-reopen cycles and compares the recovered state
  against a reference model.
- Connector compatibility smoke script
  (`tests/conformance/python/run_connector_smoke.py`) that runs a minimal real
  Aura Connector scenario against a running server.
- Benchmark baseline snapshot (`benches/baseline/v0.2.1.json`) produced by
  `auradb bench --json`, with `docs/BENCHMARKS.md`.
- JSON output for `auradb status`, `auradb doctor`, and `auradb bench`
  (`--json`), and a richer health and readiness report.

### Changed

- Improved Docker security defaults and deployment documentation; the secure
  Compose example is now the recommended deployment path.
- `auradb dump` accepts `--output` (alias of `--out`) and `auradb restore`
  accepts `--input` (alias of `--in`) for consistency with the documentation.
- Improved release-validation and operational health-check documentation.

### Fixed

- Pinned the Docker build stage to `rust:1.90-slim-bookworm` so its glibc matches
  the `debian:bookworm-slim` runtime. The unpinned `rust:1.90-slim` tag had moved
  to a newer Debian, producing an image whose binary failed at startup with a
  missing-glibc-version error.
- `auradb dump` now writes collections in dependency order so that a
  relationship target is restored before the collection that references it;
  restoring a dump with relationships no longer depends on collection ordering.
- Documentation consistency and version references across the README and the
  `docs/` tree.

## [0.2.0] - 2026-06-04

Single-node release focused on security, durability hardening, and public
usability.

### Added

- **Authentication.** Enforced static-token authentication. An `[auth]` config
  block (`enabled`, `mode = "static-token"`, `token_hash`,
  `token_hash_algorithm = "argon2id"`) gates every schema, query, mutation,
  cursor, explain, migration-estimate, and transaction operation when enabled.
  Tokens are verified against an Argon2id PHC hash with constant-time
  comparison and are never stored in plaintext. Clients may authenticate via an
  `auth_token` in the HELLO handshake or a dedicated AUTH frame (opcode `0x04`,
  returning `AuthResult` `0x84`). Only HELLO, AUTH, PING, and HEALTH are allowed
  unauthenticated. Generate a hash with `auradb auth hash-token`.
- **TLS.** Server-terminated TLS (rustls) via a `[tls]` config block (`enabled`,
  `cert_path`, `key_path`, `client_ca_path`, `require_client_cert`), including
  mutual TLS. Generate development-only certificates with
  `auradb cert generate-dev`. Clients trust the CA with `--tls-ca`.
- **Persisted indexes.** Indexes are snapshotted to an `indexes/` directory
  (`INDEX_MANIFEST.json` plus framed, CRC32-checked per-collection `.idx` files)
  at checkpoints (`auradb compact`, graceful shutdown, `auradb index rebuild`).
  On open, a snapshot loads only when its content fingerprint, schema field
  shape, and CRC all match; otherwise the engine safely rebuilds from storage.
  Persisted kinds: primary key, unique, secondary, document-path, full-text, and
  exact vector. New `auradb index check` and `auradb index rebuild` commands.
- **Document-path indexes.** Declared in a schema via
  `{ "path": "profile.company", "kind": "document_path" }`. Accelerates equality
  filters on nested document values addressed by a dotted path; reported in
  EXPLAIN as `strategy: index_lookup` with `used_index`.
- **Full-text search.** Declared via `{ "path": "body", "kind": "full_text" }`.
  Case-folded tokenizer split on non-alphanumeric boundaries with no stop-word
  removal. A `contains_text` filter matches records that contain every distinct
  query token (boolean AND), ranked by summed term frequency (not BM25). EXPLAIN
  reports `strategy: full_text_scan`; without an index it falls back to a
  tokenized `full_scan`.
- **Transaction-scoped reads.** Reads issued with a transaction id now execute
  against the transaction view — committed state overlaid with the transaction's
  own staged writes and deletes — across `find`, `filter`, `count`, `exists`,
  `explain`, vector nearest, document-path filters, full-text search,
  relationship `include`, and cursor paging. A transaction sees its staged
  inserts and updates and does not see its staged deletes (read-your-writes);
  the effects stay invisible to non-transactional readers until commit. Index
  seeding (equality, vector, full-text) is served from an overlay index built
  over the transaction view, so a staged write is never missed and a staged
  delete is never returned. This removes the prior limitation that reads inside a
  transaction ignored the transaction id and reflected only committed state.
  Covered by `crates/auradb/tests/transactions.rs`, the
  `transactional_read_sees_staged_write_over_the_wire` server test, and the
  `transaction_scoped_reads` conformance scenario.
- **Security defaults.** A non-loopback bind with auth disabled is rejected at
  startup unless `allow_insecure_bind = true` (config) or `--allow-insecure-bind`
  is passed. `auradb doctor` prints a redacted security summary.
- **CLI.** `auth hash-token`, `cert generate-dev`, `config validate`,
  `compatibility`, `index check`, `index rebuild`; `status` gains `--token`,
  `--tls-ca`, `--tls-server-name`; `server` gains `--allow-insecure-bind`.
- **Server capabilities.** New advertised capabilities: `authentication`, `tls`,
  `persisted_indexes`, `document_path_indexes`, `full_text_search`.
- **Recovery testing.** Deterministic, seeded recovery and corruption tests
  covering randomized insert/update/delete against a reference model (with and
  without checkpoint), trailing-segment truncation, mid-batch byte-flip
  detection, catalog corruption detection, and corrupt/missing index file and
  manifest repair (`crates/auradb-storage/tests/recovery.rs`,
  `crates/auradb/tests/recovery.rs`).
- **Distribution.** A published Docker image at `ghcr.io/ohswedd/auradb`
  (non-root, healthcheck, `/data` volume) and prebuilt binary release artifacts
  for Linux, macOS, and Windows targets with a `SHA256SUMS` file, produced by the
  `release.yml` workflow on `v*` tags.

### Changed

- AWP gains additive fields and opcodes (optional HELLO `auth_token`;
  `auth_required` and `authenticated` in HELLO_ACK; AUTH/AUTH_RESULT opcodes;
  `unauthenticated` and `invalid_credentials` error codes). The 44-byte framed
  header, magic, version, and checksums are unchanged and backward compatible.
- The Python conformance harness gains `--auth-token`, `--tls-ca`, and
  `--tls-server-name`, and new document-path, full-text, and EXPLAIN scenarios.
- New CI workflows: `conformance.yml` (auth disabled, auth enabled with a
  rejection check, and TLS) and `docker.yml` (build, smoke, and GHCR publish).

### Security

- Tokens are stored only as Argon2id hashes and verified in constant time;
  secrets are never logged or echoed in error frames.
- `auth.enabled = true` without `token_hash`, a malformed `token_hash`, missing
  or invalid TLS material, or `require_client_cert = true` without
  `client_ca_path` all fail startup (fail closed).
- Failed authentication increments the `auradb_auth_failures_total` metric.

## [0.1.0] - 2026-06-04

First single-node developer release.

### Added

- **Storage engine.** Append-only, checksummed segment log with a manifest,
  crash recovery (torn-tail truncation, corruption detection), and compaction.
- **Aura Wire Protocol.** Binary framed protocol with version negotiation,
  header and payload CRC32 checksums, request-id correlation, and structured
  error frames.
- **Transactions.** Buffered write sets with optimistic write and read conflict
  detection, atomic durable commit, and rollback.
- **Schema catalog.** Typed fields, primary keys, unique and secondary indexes,
  document and vector fields, relationships, and validation.
- **Query engine.** Find, filter (comparisons, `contains`, `AND`/`OR`/`NOT`),
  order/limit/offset, projection, count, exists, insert, bulk insert, update,
  delete, upsert, relationship includes, document path access, exact vector
  nearest-neighbour search, and EXPLAIN.
- **Migration impact estimation.**
- **Server-side cursors** with paging and idle-timeout reaping.
- **Server.** Async TCP listener, concurrent connections, payload limits,
  graceful shutdown, and per-connection transactions.
- **Observability.** Metrics registry (counters, gauges, latency histograms)
  with JSON and Prometheus-text export, plus structured tracing.
- **CLI.** `version`, `init`, `server`, `doctor`, `status`, `check`, `compact`,
  `dump`, `restore`, `bench`.
- **Conformance harness.** A protocol client and scenario suite, plus a Python
  harness.
- Docker support, example configuration, benchmarks, and GitHub Actions CI.

### Not yet implemented (not claimed)

Distributed clustering, replication, sharding, failover, multi-region, and Raft;
approximate (ANN/HNSW) vector indexes; BM25 full-text and hybrid fusion ranking;
serializable MVCC; enforced TLS and authentication; field-level encryption,
RBAC; time travel; and change streams. See [docs/ROADMAP.md](docs/ROADMAP.md).

[0.2.1]: https://github.com/Ohswedd/auradb/releases/tag/v0.2.1
[0.2.0]: https://github.com/Ohswedd/auradb/releases/tag/v0.2.0
[0.1.0]: https://github.com/Ohswedd/auradb/releases/tag/v0.1.0
