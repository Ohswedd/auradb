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
