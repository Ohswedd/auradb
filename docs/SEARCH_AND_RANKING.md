# Search and ranking

AuraDB v1.1.0 expands search and ranking for the single-node production line. Multi-node
remains an HA candidate preview, not production HA. The features here are additive Query IR
clauses over **Aura Wire Protocol 1** and persist additively in **storage format v2** — no
on-disk or wire format change.

## Overview

| Feature                 | Clause        | Ranking                                | Status                          |
| ----------------------- | ------------- | -------------------------------------- | ------------------------------- |
| Boolean full-text       | `contains_text` filter | summed term frequency         | unchanged (compatibility)       |
| Ranked full-text (BM25) | `text_search` | Okapi BM25                             | new in v1.1.0                   |
| Exact vector search     | `vector`      | similarity (cosine/euclidean/dot)      | correctness baseline            |
| Hybrid text + vector    | `hybrid`      | fused BM25 + vector                    | new in v1.1.0                   |
| Approximate vector (HNSW)| `vector` + `vector_ann` | exact re-rank of approx top-k | **opt-in preview (v1.2.0); not production ANN** |

A query may carry at most one of `vector`, `text_search`, or `hybrid`; setting more than one
is rejected with a structured error.

## Ranked full-text search (BM25)

The `text_search` clause ranks documents on a full-text indexed field by Okapi BM25:

```json
{
  "collection": "Doc",
  "text_search": {
    "field": "body",
    "query": "raft consensus",
    "operator": "or",
    "rank": "bm25",
    "k1": 1.2,
    "b": 0.75
  }
}
```

- **`operator`** — `"or"` (default; any term contributes to the score) or `"and"` (every
  distinct query term must be present).
- **`rank`** — `"bm25"` (default) or `"term_frequency"` (legacy summed-TF scoring).
- **`k1`/`b`** — BM25 term-saturation and length-normalization parameters; defaults are
  `k1 = 1.2`, `b = 0.75`. Documented and deterministic for reproducible tests.

BM25 uses, per full-text field: term document frequency, per-document length, average
document length, and the corpus size. These statistics are maintained in the in-memory
inverted index and persisted in the index snapshot (see [INDEXING.md](INDEXING.md)); a
snapshot written before v1.1.0 (without length statistics) rebuilds them safely on open.

The legacy `contains_text` boolean-AND predicate is **unchanged**: it still matches documents
containing every query term and ranks by summed term frequency.

## Exact vector search

The `vector` clause is exact nearest-neighbour search and remains the default and correctness
baseline. v1.2.0 adds an **opt-in approximate (HNSW) preview** (the `vector_ann` option) —
in-memory/rebuilt, re-ranked by exact similarity, and **not production ANN**. See
[VECTORS.md](VECTORS.md).

## Hybrid search

The `hybrid` clause fuses BM25 text relevance and exact vector similarity:

```json
{
  "collection": "Doc",
  "hybrid": {
    "text_field": "body",
    "text_query": "raft consensus",
    "vector_field": "embedding",
    "vector": [0.1, 0.2, 0.3],
    "top_k": 10,
    "metric": "cosine",
    "weights": { "text": 0.5, "vector": 0.5 },
    "fusion": "weighted_sum",
    "operator": "or"
  }
}
```

Fusion modes:

- **`weighted_sum`** — each signal's scores are min-max normalized to `[0, 1]`, then combined
  as `weight_text · text + weight_vector · vector`.
- **`reciprocal_rank_fusion`** — combine `weight / (60 + rank)` over each signal's ranking,
  robust to differing score scales.

Weights must be non-negative and not both zero. Ties break deterministically by record id.
Hybrid search is single-node production-supported; queried through a multi-node cluster it
follows AuraDB's preview semantics.

## Result scores

Ranked rows carry a `score` (relevance/similarity, or the fused score for hybrid). Hybrid
rows also carry `text_score` and `vector_score` components, and all ranked rows carry a
1-based `rank`. These are additive response fields; non-ranked queries omit them.

## EXPLAIN and EXPLAIN ANALYZE

`EXPLAIN` reports the chosen strategy (`full_text_bm25`, `hybrid`, `vector_exact_scan`), the
indexed field(s), ranking mode and operator, and for hybrid the fusion mode, weights, and
candidate sources. `EXPLAIN ANALYZE` additionally reports scanned/matched/returned rows,
candidate counts per signal, and timing. See [QUERY_ENGINE.md](QUERY_ENGINE.md).

## Operations

- Full-text and vector indexes survive restart, backup/restore, and compaction; BM25 length
  statistics persist in the index snapshot and rebuild safely if missing.
- `auradb index check` validates BM25 and vector index statistics.
- `auradb stats analyze` refreshes the full-text statistics the planner uses.
- `auradb search explain --input query.json [--analyze]` inspects a ranked query's plan.
- Search metrics: `search_text_queries_total`, `search_hybrid_queries_total`,
  `search_vector_queries_total`, and the `ranking_latency` histogram (see
  [OBSERVABILITY.md](OBSERVABILITY.md)).

## Multi-node (HA candidate preview)

Search and ranking are part of the **single-node production line**. Multi-node static
clustering remains an **HA candidate preview, not production HA** (no production failover, no
linearizable reads, no distributed transactions). Search behaves honestly in cluster mode:

- **Send search requests to the leader.** This is the supported, recommended path and always
  returns fresh, correct results.
- **Writes are leader-only.** A search-index write (insert/update that changes a full-text or
  vector field) sent to a follower is rejected with `not_leader`, exactly like any other
  write. The connector exposes the leader address and a bounded redirect helper.
- **Search indexes are per-node and rebuilt from the replicated log.** A follower applies the
  leader's replicated writes and rebuilds its own BM25 and vector indexes, so once a write
  has replicated the follower can rank it (validated by
  `crates/auradb-replication/tests/multi_node.rs::cluster_search_bm25_and_hybrid_after_replication`).
  The v1.2.0 query features ride the same path: aggregations/terms facets (including a BM25
  search facet) and ranked `search_page` pagination are validated on a follower after
  replication, after a leader change, after a follower restart, and after a snapshot install
  (`cluster_aggregate_and_facets_after_replication`, `cluster_search_page_after_replication`,
  `cluster_aggregate_after_leader_change`, `cluster_aggregate_after_follower_restart`,
  `cluster_aggregate_after_snapshot_install`).
- **Follower reads are eventually consistent, not linearizable.** In the preview, a follower
  serves reads (including search) from its locally replicated state; these may be briefly
  stale relative to the leader and are **not** a production read-consistency guarantee. For
  correctness and freshness, send reads to the leader.
- **A search inside a transaction is never auto-redirected.** A transaction started against a
  follower is not silently moved to the leader.

The connector search-cluster conformance
(`tests/conformance/python/run_connector_search_cluster.py`) exercises BM25 and hybrid on the
leader, leader-only search-index writes, redirect-preserves-search-query, transaction
no-auto-redirect, and search after a leader change. See [CLUSTERING.md](CLUSTERING.md) for the
general cluster read policy and [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md).
