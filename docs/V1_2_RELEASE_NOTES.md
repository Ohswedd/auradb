# AuraDB v1.2.0 release notes

**Query ergonomics and operational hardening — single-node production line, multi-node HA
candidate preview.**

AuraDB v1.2.0 expands the query surface for the single-node production line with
aggregations, terms facets, and cooperative query timeouts, without changing the production
support claim.

> AuraDB v1.2.0 expands query ergonomics (aggregations, facets, query timeouts) for the
> single-node production line.
> Multi-node remains HA candidate preview, not production HA.
>
> Approximate vector search is an opt-in **preview** in v1.2.0. Exact vector search remains
> the default and the correctness baseline. This is not production ANN.

## Highlights

- **Aggregations and terms facets** (`aggregate` read request): `count`, `min`, and `max`
  metrics and terms facets over a collection. Facets and metrics share one matched set —
  a filtered scan or, when a `text_search` clause is present, the BM25 candidate set (a
  "search facet"). A terms facet over an equality-indexed field with no residual filter is
  served straight from index posting-list lengths (no record scan); every other shape falls
  back to an honest scan, reported per-facet as `used_index`. Bucket ordering is
  deterministic: descending count, then ascending value, so `limit` truncation is stable.
- **Cooperative query timeouts** (`query_timeout`): a configurable default deadline
  (`[limits] max_query_time_ms`, default 30000) bounds every read; a per-query `timeout_ms`
  may lower (never raise) it. Scan, BM25, hybrid, exact-vector, and aggregate/facet reads
  poll the deadline and return a structured `query_timeout` error when it is exceeded. The
  timeout cancels the in-flight query only — the session and connection stay usable.
- **Approximate vector search (HNSW) preview — opt-in.** A real HNSW approximate index
  (Malkov & Yashunin layered proximity graph) is available per query via the additive
  `vector_ann` option on a vector clause (`m`, `ef_construction`, `ef_search`). Exact search
  remains the **default and correctness baseline**; the preview trades a small, tunable
  amount of recall for sub-linear query cost. The graph is built in memory from the
  authoritative exact vectors and is never persisted (storage format v2 unchanged), so it is
  always consistent with exact search. Validated by recall tests against the exact baseline
  (recall@10 ≥ 0.85 end-to-end; the algorithm itself ≥ 0.90), determinism for a fixed
  dataset, EXPLAIN reporting (`approximate`, `ef_search`), and parameter / dimension-mismatch
  rejection (the query payload is never echoed). Advertised as the
  `approximate_vector_search_preview` capability with an `auradb_ann_preview_queries_total`
  metric. **This is not production ANN.**
- **Exact vector search optimization (vector track).** Exact nearest-neighbour selection now
  uses a bounded top-k heap (`O(n log k)`) instead of sorting all candidates, with **provably
  identical** results — the ranking key (`score` descending, ties by `id` ascending) and NaN
  handling are unchanged, verified by a full-sort reference test across metrics and every `k`.
- **Stable ranked pagination over the wire.** Ranked search (BM25, hybrid, exact vector)
  can be paged by **opaque keyset cursor tokens** — via the additive `search_page` read
  request on AWP, or `Engine::search_page` in-process. Tokens are fixed-size (74 hex chars),
  bounded regardless of query size, and **carry no query payload or secrets** — only the
  continuation `(score, id)` key, a rank offset, and a non-reversible query fingerprint that
  rejects a token replayed against a different query. Vector cursors are duplicate-free even
  across concurrent writes; BM25/hybrid are stable when paged inside a transaction snapshot
  (corpus statistics shift on writes otherwise — documented). Advertised as the
  `ranked_pagination` capability, and surfaced in the connector as
  `QueryBuilder.search_pages(page_size=...)`.
- **Observability:** new `auradb_query_timeouts_total`, `auradb_facets_queries_total`, and
  `auradb_aggregation_queries_total` counters.
- **Capabilities:** `auradb compatibility` now advertises `aggregations_and_facets` and
  `query_timeouts`, and lists `facets`, `aggregations`, and `query_timeouts` under search
  features.
- **Aura Connector v0.6.0** is the paired client. Its `.timeout(ms)` query option is now
  honored end-to-end by AuraDB v1.2.0.

## Protocol and storage — unchanged

**Aura Wire Protocol 1 and storage format v2 stay frozen.** Everything new is additive:

- The aggregate request is a new `{"query": "aggregate", ...}` variant of the existing read
  opcode; older connectors never send it, and a server that predates it rejects the unknown
  `query` tag. No existing read shape changed.
- `timeout_ms` is an additive, defaulted field on the find request and the aggregate
  request. Older connectors that omit it are unaffected.
- No on-disk format change: aggregations and facets are computed from existing records and
  the existing equality/full-text indexes. No migration is required; v1.1 and v1.0 data
  open unchanged (the existing upgrade-from-older-format tests still pass).
- Backup/restore coverage: an `auradb dump` → `auradb restore` round-trip rebuilds the
  equality and full-text indexes, and the aggregate, terms-facet (including the index-backed
  path), search-facet, and query-timeout paths produce identical results on the restored
  database — covered by dedicated tests.

## Compatibility and scope

- **Single-node remains the production-supported deployment mode.**
- **Multi-node remains an HA candidate preview, not production HA.** No production automatic
  failover, dynamic membership, linearizable follower reads, distributed transactions,
  sharding, or multi-region.
- **Exact vector search remains the correctness baseline.** Approximate (HNSW) vector search
  is available as an **opt-in preview**, not production ANN.
- Aura Connector **v0.6.0** is the paired client; **v0.5.x** remains supported for existing
  features. A v0.6.0 connector against a v1.1.0 server still works for pre-1.2 features and
  receives a clear `unsupported`/unknown-tag error if it requests an aggregate.

## Not in this release (honest scope)

The v1.2.0 theme set out a larger surface than landed in this round. The following were
**not** implemented and are **not** claimed anywhere in the product; they remain on the
roadmap and exact/single-node correctness is unaffected:

(The approximate-vector HNSW preview, ranked pagination, aggregations/facets, and query
timeouts all landed this release and are therefore no longer listed here. ANN is an **opt-in
preview** — exact vector search remains the default and correctness baseline.)
- **Search/vector operational checks** in `check` / `doctor` / `index check` /
  `stats analyze` beyond the v1.1 BM25/vector validations.

(Multi-node preview coverage for the new paths is now complete: aggregations/facets and
ranked pagination are tested on a follower after replication, after a leader change, after a
follower restart, and after a snapshot install — see Highlights / the cluster tests.)

(Aggregations/terms facets, query timeouts, and ranked pagination are all fully landed
end-to-end this release — server and connector — so they are no longer listed here. The Aura
Connector v0.6.0 exposes `query.facet()/.aggregate()`, `.search_pages()`, and `.timeout()`.)

## Upgrade

In-place. v1.1 and v1.0 data directories open unchanged; no index rebuild is required for
the new features (they read existing indexes). The default query deadline
(`max_query_time_ms = 30000`) applies on upgrade — set it to `0` to disable, or raise it for
workloads with intentionally long reads.
