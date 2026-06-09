# AuraDB Roadmap

This roadmap tracks planned and candidate work. It is a statement of direction,
not a delivery commitment, and it is **not a changelog**. Completed release
history lives in [CHANGELOG.md](../CHANGELOG.md) and the per-release notes under
`docs/`.

## Current stable baseline

The roadmap is written against the latest shipped release.

- **AuraDB v1.3.1** is the current stable release; single-node remains the
  production-supported deployment mode.
- **Aura Connector v0.7.0** is the tested paired client line.
- **AWP 1**, **storage format v2**, and **index snapshot format version 1** are the
  frozen v1.x compatibility baseline — see [COMPATIBILITY.md](COMPATIBILITY.md).

## Current product stance

This stance is the baseline the roadmap builds on; it is context, not a list of
deliverables.

- **Single-node is the production-supported deployment mode.**
- **Multi-node is an HA candidate preview**, not a production HA guarantee — see
  [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md).
- **AWP 1 and storage format v2 are the v1 compatibility baseline**, frozen for
  the v1.x line — see [COMPATIBILITY.md](COMPATIBILITY.md).
- **Search and ranking** (BM25 full-text and hybrid text+vector) are part of the
  single-node production line — see [SEARCH_AND_RANKING.md](SEARCH_AND_RANKING.md).
- **Exact vector search is the correctness baseline.** Approximate
  (HNSW) vector search is available as an **opt-in preview** (v1.2.0, with durable
  lifecycle metadata and an `ann_fallback` policy added in v1.3.0), not production
  ANN. The graph is still rebuilt in memory from the exact vectors and is never
  persisted.

For what is and is not supported today, see [SUPPORT_POLICY.md](SUPPORT_POLICY.md).

## How to read this roadmap

- `[ ]` planned / open
- `[~]` being evaluated, partial, or under investigation
- `[x]` completed — used only sparingly, for context, never as release history

Items are actionable work, not promises. Where the path is uncertain we say so
("investigate", "evaluate", "candidate", "future work", "not yet committed").

## Target: AuraDB v1.4.0 — Production operability and search quality

The next big release strengthens the **production-supported single-node path**,
improves **search and vector quality evidence**, and raises **operator confidence**.
It is deliberately *not* a production-HA, production-ANN, sharding, multi-region, or
distributed-database release; those remain future work (see "Later / not v1.4.0 by
default" and "Not currently planned for immediate work"). It pairs with **Aura
Connector v0.8.0**.

Planned scope (detailed per category in the sections below):

- **Production single-node drills:** disk-full drill, I/O-error drill,
  backup/restore rehearsal on larger state, more upgrade fixtures across released
  versions, and a restore/rollback rehearsal.
- **Search quality:** a relevance dataset format, search regression fixtures, BM25
  (k1 / b) tuning guidance, a hybrid ranking calibration harness, and
  analyzer/tokenizer planning (or a first safe implementation if scoped).
- **Vector evidence:** larger exact-vs-ANN evaluation datasets, recall/latency
  report examples, ANN health/reporting improvements, and exact-fallback evidence —
  with **no production ANN claim**.
- **Operator confidence:** SLO templates, incident runbook templates,
  doctor/check/report improvements where needed, and published-image smoke
  expansion.
- **Release engineering:** SBOM/signing investigation (or a first implementation if
  feasible), a release rollback drill, and artifact-verification improvements.

The category sections below carry the per-item tracking; this section is the
v1.4.0 lens over them.

## Production single-node hardening

Single-node is the supported production mode, so most operability investment
lands here first.

- [ ] Longer single-node soak runs on maintained hardware.
- [ ] Disk-full and I/O-error drills with documented recovery behavior.
- [ ] Backup encryption workflow examples.
- [ ] More upgrade rehearsal fixtures across released versions.
- [ ] Operator SLO templates.
- [ ] Production incident / recovery runbook templates.

## Search and ranking

BM25 and hybrid ranking ship today; this category covers relevance quality and
operability of search, not the existence of search.

- [ ] Search relevance evaluation datasets.
- [ ] BM25 parameter (k1 / b) tuning guidance.
- [~] Highlight / snippet support — evaluate.
- [x] Faceting and aggregations over result sets — shipped in v1.2.0 (`aggregate`
  request: `count`/`min`/`max` metrics and terms facets, including BM25 search
  facets, with an index-backed facet path and honest scan fallback).
- [x] Search pagination stability under concurrent writes — stable ranked-cursor
  *tokens* (keyset pagination) shipped in v1.2.0 end-to-end: `Engine::search_page`,
  the `search_page` AWP request (`ranked_pagination` capability), and the connector
  `QueryBuilder.search_pages()` helper. The server advertises a `cursor_resume`
  capability so an opaque resume token can be persisted and resumed across processes.
- [ ] Query-time analyzers / tokenizers.
- [ ] Synonyms or custom analyzers.
- [ ] Hybrid ranking calibration tooling.

## Vector search

Exact vector search is the correctness baseline; approximate search shipped as an
opt-in preview in v1.2.0 and is hardening toward production-grade.

- [x] ANN / HNSW prototype behind the `VectorIndex` seam — shipped in v1.2.0 as an
  opt-in, recall-tested preview (`vector_ann`); exact remains the default/baseline.
- [x] HNSW preview durable lifecycle metadata and exact-fallback policy — shipped in
  v1.3.0: the index snapshot records additive per-field lifecycle metadata (field,
  dimension, vector count, generation marker) so the preview's status survives
  restarts, and an `ann_fallback` policy (`exact` default / `error`) governs queries
  when the preview is unavailable. The graph itself is still **rebuilt in memory from
  the exact vectors on first use and never persisted** — this is not production ANN.
- [x] Recall / latency evaluation harness for ANN vs exact — shipped in v1.3.0 as the
  `auradb vector eval` operator command (recall@k and latency over a deterministic
  query set; dataset- and machine-specific numbers, never a universal claim).
- [ ] **Persistent / incremental HNSW graphs** — the preview rebuilds the in-memory
  graph from the exact vectors; persistence and incremental maintenance are the next
  step toward production ANN (not v1.4.0 by default).
- [ ] ANN-specific `index check` / `stats analyze` (graph health, recall sampling).
- [ ] Larger exact-vs-ANN evaluation datasets and recall/latency report examples.
- [ ] Exact-vs-ANN comparison tooling.
- [~] Vector quantization / memory-planning research for large embedding sets.
- [ ] Larger vector dataset tests.

## Query engine and planner

The cost-based planner exists; this category is about sharper estimates and
operator control.

- [ ] Better cardinality estimates.
- [ ] Histograms or richer column statistics.
- [ ] Multi-field index planning.
- [x] `EXPLAIN ANALYZE` query-profile fields — shipped in v1.3.0 (additive `plan_id`,
  `deadline_ms`, and `timeout_checked` alongside the measured counts and timings; the
  query payload is never echoed). More diagnostics remain open below.
- [ ] More `EXPLAIN ANALYZE` diagnostics.
- [x] Query timeout / cancellation controls — shipped in v1.2.0 as a cooperative
  deadline (`[limits] max_query_time_ms` default + per-query `timeout_ms`,
  structured `query_timeout`). Preemptive mid-operation cancellation remains out
  of scope; the check is cooperative.
- [x] Aggregations — Query IR shape settled and shipped in v1.2.0 (`aggregate`).
- [x] GROUP BY analytics — shipped in v1.3.0 as an additive `group_by` clause on the
  `aggregate` request (single scalar group key; per-group `count`/`min`/`max`/`avg`;
  deterministic ordering; `group_limit` / `group_count_total`).

## Storage and durability

Storage format v2 is frozen for v1; work here is validation and operability
within that format.

- [ ] Disk-full drills.
- [ ] I/O-error injection tests.
- [~] Incremental backup research.
- [ ] Snapshot / restore scaling under large state.
- [ ] More corruption-recovery drills.
- [ ] Compaction scheduling improvements.

## Security and operations

Enforced auth and TLS ship today; these are additive controls operators have
asked about, none of which are committed yet.

- [ ] Audit logging.
- [~] RBAC or scoped tokens — investigate.
- [ ] Key-management integration examples.
- [ ] Certificate rotation automation.
- [ ] Security hardening profiles.
- [ ] Production monitoring dashboards.

## Multi-node HA candidate preview

Multi-node is a preview today. These items are the gate between "candidate
preview" and any future production-HA decision; the full criteria live in
[HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) and
[PRODUCTION_READINESS.md](PRODUCTION_READINESS.md).

- [ ] Cross-host chaos testing (beyond loopback).
- [ ] Longer multi-node soak with zero data loss / duplicate apply.
- [ ] Chunked / streaming snapshot transfer for large state.
- [ ] Operator SLO definitions and non-goals.
- [ ] External dogfood period.
- [ ] Safer public peer deployment docs.
- [~] Dynamic membership design — not yet committed.
- [~] Linearizable read design — not yet committed.
- [ ] Production-HA decision gate.

## Developer experience and ecosystem

- [ ] More worked examples.
- [ ] Better local development environment.
- [ ] Migration tooling.
- [~] Admin UI / dashboard exploration.
- [~] Additional language clients — candidate, if demand warrants.

## Release engineering

- [x] Cluster search-analytics release smoke — shipped in v1.3.0 and corrected in
  v1.3.1 (the smoke now resolves the leader by each node's self-reported role and
  waits for a genuine leader change during the failover drill). It is a controlled
  single-host preview drill, **not production HA proof**.
- [ ] Reproducible build improvements.
- [ ] SBOM / signing investigation, then a first implementation if feasible.
- [ ] More published-image smoke coverage.
- [ ] Release rollback drills.
- [ ] Artifact-verification improvements.

## Later / not v1.4.0 by default

These are real future directions, but they are explicitly **out of v1.4.0 scope**
unless intentionally re-scoped. They do not weaken the v1.4.0 production-single-node
and search-quality focus.

- Production-HA decision gate for multi-node.
- Cross-host multi-node chaos soak (beyond loopback).
- Dynamic membership.
- Linearizable follower reads.
- Production ANN.
- Persisted / incremental HNSW graph.
- Sharding.
- Multi-region.

## Not currently planned for immediate work

These are listed so the boundary is explicit. They are not promised and not
implied by any other section.

- Production HA claim for multi-node.
- Sharding.
- Multi-region deployment.
- Distributed transactions.
- Managed cloud service.
- Kubernetes operator.
