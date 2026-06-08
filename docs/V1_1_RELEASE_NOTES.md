# AuraDB v1.1.0 release notes

**Search and ranking — single-node production line, multi-node HA candidate preview.**

AuraDB v1.1.0 is the first larger post-1.0 release. It expands search and ranking for the
single-node production line and adds first-class connector support, without changing the
production support claim.

> AuraDB v1.1.0 expands search and ranking features for the single-node production line.
> Multi-node remains HA candidate preview, not production HA.
>
> Vector search remains exact in v1.1.0. ANN/HNSW remains unsupported.

## Highlights

- **BM25 ranked full-text search** (`text_search` clause): Okapi BM25 relevance ranking over
  full-text indexed fields, with `or`/`and` term operators and tunable `k1`/`b`.
- **Hybrid text + vector ranking** (`hybrid` clause): deterministic fusion of BM25 text
  relevance and exact vector similarity (`weighted_sum` or `reciprocal_rank_fusion`).
- **Planner awareness** of ranked text and hybrid retrieval, with stable, additive EXPLAIN /
  EXPLAIN ANALYZE shape.
- **CLI**: search capabilities in `auradb compatibility`, BM25/vector validation in `auradb
  index check`, full-text stat refresh in `auradb stats analyze`, and a new `auradb search
  explain [--analyze]`.
- **Observability**: search query counters and a ranking-latency histogram.
- **Aura Connector v0.5.0**: first-class `search_text`, `search_vector`, `search_hybrid`
  APIs, typed result scores, and capability negotiation.

## Compatibility and scope

| Aspect             | v1.1.0                                                                 |
| ------------------ | --------------------------------------------------------------------- |
| Production support | Single-node, as in v1.0.x — unchanged                                 |
| Multi-node         | HA candidate preview, not production HA — unchanged                   |
| Aura Wire Protocol | AWP 1 (new clauses are additive Query IR / response fields)           |
| Storage format     | v2 (BM25 length stats persist additively in the index snapshot)       |
| Aura Connector     | tested 0.5.0, supported 0.5.x                                         |
| Vector search      | exact only; ANN/HNSW not implemented                                  |

In-place upgrade from v1.0.x is supported: existing data opens unchanged and new search
statistics rebuild safely on open. See [UPGRADING.md](UPGRADING.md).

## What's new in detail

See [SEARCH_AND_RANKING.md](SEARCH_AND_RANKING.md) for the full clause reference, BM25
formulation, fusion modes, score fields, and EXPLAIN shapes. See
[QUERY_ENGINE.md](QUERY_ENGINE.md), [INDEXING.md](INDEXING.md), and [VECTORS.md](VECTORS.md)
for engine, index, and vector details.

## Multi-node search behavior (preview)

Search and ranking are single-node production-supported. In the multi-node preview, send
search to the leader; search-index writes are leader-only; a follower rebuilds its BM25 and
vector indexes from the replicated log; and follower reads (including search) are eventually
consistent and not linearizable. This is validated by the connector search-cluster
conformance and an engine-level replication test. See
[SEARCH_AND_RANKING.md](SEARCH_AND_RANKING.md) and [CLUSTERING.md](CLUSTERING.md).

## Known limitations (unchanged honesty)

- No approximate (ANN/HNSW) vector search. Exact vector search is the correctness baseline.
- No production high availability, automatic failover, dynamic membership, sharding,
  multi-region, distributed transactions, or a production read-consistency guarantee
  (followers serve only eventually-consistent, non-linearizable reads; send reads to the
  leader). Multi-node is an HA candidate preview.
