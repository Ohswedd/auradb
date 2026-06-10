# AuraDB v1.4.0 release notes

**Theme: production operability and search quality.**

AuraDB v1.4.0 is a minor release on the v1 single-node production line. It does two
things: it makes operating a single node in production easier to rehearse and verify,
and it makes search ranking quality measurable with a committed, offline evaluation
harness. It introduces **no new wire protocol, storage format, or query-engine
surface** — everything shipped here is operability and evaluation **tooling**.

AuraDB v1.4.0 is paired with **Aura Connector v0.8.0** (compatible 0.8.x). Connector
v0.7.x/0.6.x/0.5.x remain supported for the existing feature set; because there is no
wire change, a v0.7.x connector remains fully compatible with AuraDB 1.4.0.

See [COMPATIBILITY.md](COMPATIBILITY.md), [SUPPORT_POLICY.md](SUPPORT_POLICY.md),
[OPERATIONS.md](OPERATIONS.md), [SEARCH_AND_RANKING.md](SEARCH_AND_RANKING.md),
[TESTING.md](TESTING.md), and [BENCHMARKS.md](BENCHMARKS.md).

## Shipped

Production operability:

- **Single-node production drill harness**
  (`scripts/smoke_single_node_production_drills.sh`).
- **Backup/restore rehearsal and rollback drill** documentation and gates — restore
  into a fresh directory and roll back to a known-good snapshot.
- **Disk-space preflight and a safe injected I/O-error drill** — surfaces I/O errors
  cleanly without corrupting data.
- **Machine-readable drill report** (`report.json`) with an `overall` pass/fail field.

Search quality:

- **Search relevance dataset format** — a documented corpus/queries/qrels JSONL format
  (see [`fixtures/relevance/README.md`](../fixtures/relevance/README.md)).
- **Small committed relevance fixture** under `fixtures/relevance/`.
- **`auradb search eval`** — an offline relevance-evaluation command.
- **MRR@k / NDCG@k / Recall@k** ranked-retrieval quality metrics.
- **BM25 `k1`/`b` evaluation guidance** for tuning the full-text ranker.
- **Hybrid calibration harness** for the text+vector blend.
- **`vector_exact` evaluation mode** alongside `bm25` and `hybrid`.

## Unchanged

- **AWP 1** — the Aura Wire Protocol is unchanged.
- **Storage format v2** — unchanged; v1.3.x and earlier data open with no rebuild.
- **Index snapshot format version 1** — unchanged.
- **Single-node is production-supported** and remains the recommended production mode.
- **Multi-node HA is a candidate preview only** — not production HA, no production
  automatic failover.
- **HNSW/ANN is an opt-in preview only** — not production ANN. The graph is never
  persisted; it is rebuilt in memory from the exact vectors on use.
- **Exact vector search is the default and correctness baseline.**
- **Query timeouts remain cooperative.**

## Known limitations

- The committed relevance fixtures are **regression signals for the shipped datasets,
  not universal benchmarks** and not a guarantee of relevance on arbitrary corpora.
- The disk-full drill is a **safe preflight / injected-failure style** check — it does
  **not** actually fill the disk.
- The **HNSW graph is not persisted**; it is rebuilt in memory on use.
- **No production ANN.**
- **No production HA.**

## Upgrade

No migration is required from v1.3.x. AWP 1, storage format v2, and index snapshot
format version 1 are unchanged, so existing single-node data directories open as-is.
Upgrade the paired client to Aura Connector v0.8.0 to pick up the new client-side
ergonomics (connection profiles, search-eval report parsing helpers, capability
helpers); v0.7.x continues to work unchanged.
