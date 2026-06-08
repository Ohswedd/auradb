# AuraDB Roadmap

This roadmap tracks planned and candidate work. It is a statement of direction,
not a delivery commitment, and it is **not a changelog**. Completed release
history lives in [CHANGELOG.md](../CHANGELOG.md) and the per-release notes under
`docs/`.

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
  (ANN / HNSW) vector search is not implemented.

For what is and is not supported today, see [SUPPORT_POLICY.md](SUPPORT_POLICY.md).

## How to read this roadmap

- `[ ]` planned / open
- `[~]` being evaluated, partial, or under investigation
- `[x]` completed — used only sparingly, for context, never as release history

Items are actionable work, not promises. Where the path is uncertain we say so
("investigate", "evaluate", "candidate", "future work", "not yet committed").

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
- [ ] Faceting and aggregations over result sets.
- [ ] Search pagination stability under concurrent writes.
- [ ] Query-time analyzers / tokenizers.
- [ ] Synonyms or custom analyzers.
- [ ] Hybrid ranking calibration tooling.

## Vector search

Exact vector search is the correctness baseline; approximate search is research,
not a committed feature.

- [ ] ANN / HNSW design and prototype behind the existing `VectorIndex` seam.
- [ ] Recall and latency benchmark harness.
- [ ] Exact-vs-ANN comparison tooling.
- [~] Vector quantization / memory-planning research for large embedding sets.
- [ ] Larger vector dataset tests.

## Query engine and planner

The cost-based planner exists; this category is about sharper estimates and
operator control.

- [ ] Better cardinality estimates.
- [ ] Histograms or richer column statistics.
- [ ] Multi-field index planning.
- [ ] More `EXPLAIN ANALYZE` diagnostics.
- [ ] Query timeout / cancellation controls.
- [~] Aggregations — evaluate scope and Query IR shape.

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

- [ ] Reproducible build improvements.
- [ ] SBOM / signing improvements.
- [ ] More published-image smoke coverage.
- [ ] Release rollback drills.

## Not currently planned for immediate work

These are listed so the boundary is explicit. They are not promised and not
implied by any other section.

- Production HA claim for multi-node.
- Sharding.
- Multi-region deployment.
- Distributed transactions.
- Managed cloud service.
- Kubernetes operator.
