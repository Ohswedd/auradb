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
