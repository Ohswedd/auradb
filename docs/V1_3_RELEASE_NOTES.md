# AuraDB v1.3.0 release notes

**Query ergonomics, vector-preview durability, and query observability — single-node
production line, multi-node HA candidate preview.**

AuraDB v1.3.0 extends the query surface for the single-node production line with GROUP BY
aggregations and richer query profiling, and matures the opt-in approximate-vector preview
with durable lifecycle metadata and an explicit exact-fallback policy — all without changing
the production support claim.

> AuraDB v1.3.0 adds GROUP BY aggregations and EXPLAIN ANALYZE query-profile fields to the
> single-node production line.
> Multi-node remains an HA candidate preview, not production HA.
>
> Approximate vector search remains an opt-in **preview**. Exact vector search remains the
> default and the correctness baseline. This is not production ANN.

## Highlights

- **GROUP BY aggregations** (`aggregate` read request). The aggregate request gains an
  additive `group_by` clause: a single scalar field over which the matched set is bucketed,
  with per-group `count`, `min`, `max`, and the new `avg` metric. Groups ride the **same
  matched set** as facets and metrics, so they compose for free with filters and — when a
  `text_search` clause is present — BM25 search-candidate scoping. Records whose group key is
  null or missing are excluded from grouping (the top-level `matched` count is unchanged).
  Groups are ordered deterministically by **descending count, then ascending key**; an
  optional `group_limit` (default 1000) truncates the result while `group_count_total` reports
  the full distinct-group count so truncation is always visible. `avg` considers only `Int`
  and `Float` values and yields null when a group has no numeric value; a non-scalar or
  unknown group field is rejected with `invalid_request`. The grouped result rides inside the
  existing aggregate request/response — additive, with no Aura Wire Protocol change.
- **Approximate-vector (HNSW) preview durability and exact fallback.** The approximate graph
  is still **never persisted** — it rebuilds in memory from the authoritative exact vectors on
  first use, so it is always consistent with exact search and **storage format v2 is
  unchanged**. What is new is durable **lifecycle metadata**: the index snapshot now records,
  additively per vector field, the field, dimension, indexed-vector count, and a generation
  marker, so an operator can see the preview's status across restarts. A new
  `ann_fallback` policy governs what happens when the preview is unavailable for a query
  (for example, below `ANN_PREVIEW_MIN_VECTORS = 16` indexed vectors, where the navigable
  graph degenerates and offers no benefit over the exact scan): the default `exact` runs exact
  search and reports the fallback, while `error` returns a structured error for callers that
  specifically require approximate semantics. `EXPLAIN` reports the resolved `vector_mode`
  (`exact`, `ann_preview`, or `exact_fallback`). **This is not production ANN.**
- **ANN recall/latency evaluation harness** (`auradb vector eval`). A new operator command
  measures the approximate preview's recall@k and latency against the exact baseline over a
  deterministic query set and emits JSON. Flags: `--data-dir`, `--collection`, `--field`,
  `--queries` (a file with one JSON array of floats per line), `--k`, `--metric`,
  `--ef-search`, and `--json`. The report carries `collection`, `field`, `metric`, `queries`,
  `k`, `ef_search`, `mean_recall_at_k`, `min_recall_at_k`, `exact_latency_ms_p50`, and
  `ann_latency_ms_p50`. Every number is measured on the given dataset and machine — it is a
  same-machine diagnostic, **not** a universal recall or performance claim — and the query
  vectors are never echoed into the report.
- **EXPLAIN ANALYZE query-profile enrichment.** The ANALYZE output gains additive profile
  fields for debugging alongside the existing measured counts and timings: a deterministic
  `plan_id` (the same query shape yields the same id across runs, so profiles group cleanly in
  dashboards), the cooperative `deadline_ms` in effect (`null` when the query carried no
  timeout), and a `timeout_checked` flag indicating a deadline was active and polled. All
  additions are additive and optional; the query payload is never echoed into the plan, and
  timeouts remain cooperative.

## Protocol and storage — unchanged

**Aura Wire Protocol 1, storage format v2, and the index snapshot format version (1) stay
frozen.** Everything new is additive:

- GROUP BY is an additive `group_by` (and optional `group_limit`) field on the existing
  `aggregate` read request, and the grouped result is an additive `groups` object on the
  aggregate response — omitted from the wire when absent, so older connectors are unaffected.
  No existing read or response shape changed.
- The approximate-preview lifecycle metadata is an additive `hnsw` field inside the existing
  CRC-protected index snapshot frame. The on-disk index format version stays at 1: older
  readers ignore the field and snapshots written before v1.3.0 deserialize it as empty. The
  approximate graph itself is still not persisted — it rebuilds in memory on first use — so
  the storage log format and the manifest `format_version` (2) are unchanged.
- The EXPLAIN ANALYZE profile fields (`plan_id`, `deadline_ms`, `timeout_checked`) are
  additive JSON on the existing ANALYZE object, reached through the raw Query IR
  `"analyze": true` flag on the existing `Explain` opcode — no new opcode and no protocol
  break.
- `auradb vector eval` is a new operator command; it adds no wire shape and reads only the
  existing exact-vector and opt-in approximate-preview query paths.
- No on-disk format change and no migration: GROUP BY reads existing records and indexes, and
  v1.2, v1.1, and v1.0 data directories open unchanged with no required index rebuild (the
  approximate graph rebuilds in memory on first use as before).

## Compatibility and scope

- **Single-node remains the production-supported deployment mode.**
- **Multi-node remains an HA candidate preview, not production HA.**
- **Exact vector search remains the correctness baseline.** Approximate (HNSW) vector search
  is available as an **opt-in preview**, not production ANN. The graph is never persisted; it
  rebuilds in memory from the exact vectors on use.
- Aura Connector **v0.7.0** is the paired client (and compatible **v0.7.x**); **v0.6.x**
  remains supported for the existing feature set. The new capabilities — GROUP BY
  aggregations, the EXPLAIN ANALYZE profile fields, and the approximate-preview lifecycle and
  `ann_fallback` policy — are all additive, so a connector that predates them simply omits and
  ignores them. AuraDB v1.3.0 is **backward compatible with Aura Connector 0.6.1** for the
  existing features.

## Known limitations

Honest limitations carried by this release (unchanged scope boundaries):

- **Multi-node is an HA candidate preview, not production HA.** No production automatic
  failover, no linearizable follower reads (follower reads/search are eventually consistent),
  no distributed transactions, and no dynamic membership, sharding, or multi-region.
  Single-node remains the recommended production mode.
- **Approximate (HNSW) vector search is an opt-in preview, not production ANN.** Exact vector
  search remains the default and the correctness baseline.
- **The approximate graph is never persisted; it rebuilds in memory on use.** Only the
  lifecycle metadata (field, dimension, vector count, generation marker) is durable. Below
  `ANN_PREVIEW_MIN_VECTORS = 16` indexed vectors the preview is unavailable and the
  `ann_fallback` policy applies (default `exact`).
- **The eval harness does not emit a candidate-count average.** Per-query HNSW
  candidates-visited is not yet surfaced through the query result, so `candidate_count_avg` is
  not part of the `auradb vector eval` report in this release; recall and latency are.
- **`auradb vector eval` numbers are dataset- and machine-specific.** They are a same-machine
  diagnostic for tuning `ef_search`, never a universal recall or performance claim.
- **Query timeouts are cooperative, not preemptive.** Reads poll the deadline on their
  candidate/scan loop, so cancellation is "soon after" the deadline rather than instantaneous.
  The EXPLAIN ANALYZE `deadline_ms`/`timeout_checked` fields report this cooperative deadline,
  not a preemptive one.

## Upgrade

In-place. v1.2, v1.1, and v1.0 data directories open unchanged; no index rebuild is required.
The approximate-preview lifecycle metadata is written additively at the next checkpoint
(flush, compaction, graceful shutdown, or `auradb index rebuild`); the approximate graph
itself rebuilds in memory on first use as before. See [UPGRADING.md](UPGRADING.md).
