# Vectors

AuraDB supports fixed-dimension float vector fields and exact nearest-neighbour
search.

## Schema

A vector field declares its dimension:

```json
{"name": "embedding", "field_type": {"kind": "vector", "dim": 1536}}
```

On the wire and in records, vector values use the extension encoding
`{"$vector": [0.1, 0.2, ...]}`. Inserting a vector whose length differs from the
declared dimension is rejected with a schema violation.

## Metrics

- **cosine** - cosine similarity in `[-1, 1]` (higher is more similar).
- **euclidean** - L2 distance (closer is more similar).
- **dot_product** - raw dot product (higher is more similar).

Each result row carries a `score` (higher = more similar) and the index also
computes a `distance` (lower = more similar).

## Search

A find with a vector clause ranks all stored vectors of the field by the metric
and returns the top `k`, then applies any additional filter:

```json
{
  "collection": "Doc",
  "vector": {"field": "embedding", "query": [ ... ], "k": 10, "metric": "cosine"},
  "filter": {"type": "compare", "field": "status", "op": "eq", "value": "published"}
}
```

## Honesty

Search is **exact** (full scan), which is correct and predictable but O(n) per
query. Approximate nearest-neighbour (HNSW/IVF), quantization, and disk-backed
vector indexes are **not implemented and not claimed**. The `VectorIndex` trait
is the seam where an ANN index can be added with recall and persistence tests.

## Benchmark

`crates/auradb/benches/vector.rs` measures exact top-10 search over 10k
64-dimensional vectors with `cargo bench -p auradb`.

## v1.1.0: still exact, plus hybrid

Vector search remains **exact** in v1.1.0. ANN/HNSW remains unsupported; exact search is the
correctness baseline. v1.1.0 adds hybrid retrieval that fuses BM25 text relevance with exact
vector similarity — see [SEARCH_AND_RANKING.md](SEARCH_AND_RANKING.md). The `VectorIndex`
trait still leaves room for a future approximate implementation without changing the query
engine.

## v1.2.0: opt-in approximate (HNSW) preview

v1.2.0 adds a real **approximate** nearest-neighbour index (HNSW — a layered proximity graph,
Malkov & Yashunin) as an **opt-in preview**. **Exact search remains the default and the
correctness baseline; this is not production ANN.**

- **Opt-in per query.** Set the `vector_ann` option alongside a vector clause. Absent → exact
  search (unchanged). Parameters: `m` (graph degree), `ef_construction` (build beam width),
  `ef_search` (query beam width — higher trades cost for recall). All are optional with sane
  defaults (`m=16`, `ef_construction=200`, `ef_search=64`), and validated (out-of-range
  values are rejected with a structured error).
- **Recall / latency tradeoff.** The preview returns approximate top-k, re-ranked by exact
  similarity so a perfect-recall result is identical to exact. Recall is tunable via
  `ef_search`; the recall tests assert ≥ 0.90 (algorithm) and ≥ 0.85 (end-to-end) against the
  exact baseline.
- **Built from the exact vectors, never persisted.** The graph is a derived in-memory query
  accelerator, rebuilt from the authoritative exact vectors and cleared whenever they change,
  so it is never stale and **storage format v2 is unchanged**. The graph is currently rebuilt
  per index instance (on first ANN query after a write); persistent/incremental graphs and
  ANN-specific `index check` / `stats analyze` are future work as the preview matures.
- **Diagnostics.** `EXPLAIN` reports `approximate: true` and the `ef_search` used; the server
  advertises the `approximate_vector_search_preview` capability and increments
  `auradb_ann_preview_queries_total`. Dimension-mismatch errors never echo the query vector.

The connector exposes the preview via `search_vector(..., approximate=...)` — see the Aura
Connector `SEARCH_AND_RANKING.md`.

## v1.3.0: preview durability, exact fallback, and an eval harness

v1.3.0 matures the opt-in approximate (HNSW) preview without changing its scope: **exact
search remains the default and the correctness baseline; this is still not production ANN.**

- **Durable lifecycle metadata, graph still never persisted.** The approximate graph itself
  is still **never written to disk** — it rebuilds in memory from the authoritative exact
  vectors on first use, so it is always consistent with exact search and **storage format v2
  is unchanged**. What is now durable is per-vector-field **lifecycle metadata** in the index
  snapshot: the field, its dimension, the indexed-vector count, and a generation marker (the
  collection content fingerprint at snapshot time). This is an additive field inside the
  existing CRC-protected snapshot frame; the index snapshot format version stays at 1, and
  snapshots written before v1.3.0 load with empty metadata. The metadata lets an operator see
  the preview's status across restarts and lets the engine recognise a dimension change or a
  stale generation that requires a rebuild.
- **Exact-fallback policy (`ann_fallback`).** A vector query that requests the preview may set
  `ann_fallback` to choose what happens when the preview is unavailable for that query:
  `exact` (the default) runs exact search and reports the fallback in the plan; `error`
  returns a structured error for callers that specifically require approximate semantics. Exact
  search is always available and always correct, so the default falls back to it.
- **Minimum-dataset threshold (`ANN_PREVIEW_MIN_VECTORS = 16`).** Below this many indexed
  vectors the navigable graph degenerates toward full connectivity and offers no benefit over
  the exact scan (which is also cheaper and is the correctness baseline), so the preview is
  treated as unavailable and the `ann_fallback` policy applies. With `ann_fallback = exact`
  the result is byte-for-byte the exact top-k.
- **`vector_mode` in EXPLAIN.** `EXPLAIN` reports the resolved vector execution mode for a
  query: `exact` (the baseline; no approximate request), `ann_preview` (the preview served the
  query), or `exact_fallback` (approximate was requested but the preview was unavailable, so
  exact served it). An `exact_fallback` plan also sets `exact_fallback: true`. Execution and
  EXPLAIN share one resolution path, so the two never disagree.
- **Status reporting.** The search-index report (`auradb index check`, the engine's search
  index report) describes each vector field's preview status: `ready_on_use` when the field
  clears the threshold (the graph builds in memory on the first preview query), or
  `exact_only_below_threshold` below it, alongside the durable generation marker.

### Recall/latency evaluation: `auradb vector eval`

`auradb vector eval` measures the approximate preview's recall@k and latency against the exact
baseline over a deterministic set of query vectors, and emits JSON:

```bash
auradb vector eval --data-dir .local/auradb --collection Doc --field embedding \
  --queries queries.jsonl --k 10 --metric cosine --ef-search 64 --json
```

The query file holds **one JSON array of floats per line** (one query vector per line). The
report carries `collection`, `field`, `metric`, `queries`, `k`, `ef_search`,
`mean_recall_at_k`, `min_recall_at_k`, `exact_latency_ms_p50`, and `ann_latency_ms_p50`.
Every number is **measured on the given dataset and machine** — it is a same-machine
diagnostic for tuning `ef_search`, **not** a universal recall or performance claim — and the
query vectors are never echoed into the report. A per-query candidate-count average is not
emitted in this release (per-query HNSW candidates-visited is not yet surfaced through the
query result). See [CLI.md](CLI.md) and [BENCHMARKS.md](BENCHMARKS.md).
