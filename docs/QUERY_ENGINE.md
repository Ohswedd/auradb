# Query Engine

`auradb-query` defines the Query IR and executes it over a `DataSource` (the
engine).

## Query IR

The IR is a transparent JSON model (the documented Aura-Connector compatibility
layer). Reads use `ReadRequest`:

- `Find(FindQuery)` - `collection`, optional `filter`, optional `vector`,
  `order_by`, `limit`, `offset`, `projection`, `includes`, optional `timeout_ms`.
- `Count(CountQuery)` and `Exists(ExistsQuery)` - `collection` + optional filter.
- `Aggregate(AggregateQuery)` - `collection`, optional `filter`, optional
  `text_search` (search-facet scope), `facets`, `metrics`, optional `timeout_ms`
  (added in v1.2.0).
- `SearchPage(SearchPageRequest)` - `{find, page_size, cursor?}`: page a ranked
  search by stable cursor token (added in v1.2.0).

Writes use `Mutation`: `Insert`, `BulkInsert`, `Update`, `Delete`, `Upsert`.

## Aggregations and facets (v1.2.0)

The `aggregate` read request computes metrics and terms facets over one matched
set. Metrics are `count`, `min`, and `max` (`min`/`max` take a field); a facet
requests a scalar `field` and an optional bucket `limit` (default 10).

```json
{
  "query": "aggregate",
  "collection": "products",
  "filter": {"type": "compare", "field": "in_stock", "op": "eq", "value": true},
  "facets": [{"field": "category", "limit": 10}, {"field": "brand"}],
  "metrics": [{"op": "count"}, {"op": "min", "field": "price"}, {"op": "max", "field": "price"}]
}
```

The matched set is a filtered scan, or — when a `text_search` clause is present —
the BM25 candidate set (a *search facet*); the filter, if any, is applied as a
residual. A terms facet over an equality-indexed field with **no** residual filter
and **no** search scope is served directly from index posting-list lengths (no
record scan); every other shape falls back to a scan. Each facet reports
`used_index` so the access path is honest. Buckets are ordered by **descending
count, then ascending value**, then truncated to `limit`, so `limit` paging is
deterministic. Faceting a non-scalar (vector/array/object) field, or an unknown
field, is rejected with `invalid_request`.

The result is `{collection, matched, scanned, filter_present, search_scoped,
metrics: [{op, field?, value}], facets: [{field, used_index, buckets: [{value,
count}]}]}`.

## Query timeouts (v1.2.0)

Every read is bounded by a cooperative deadline. The server default is `[limits]
max_query_time_ms` (default 30000ms; `0` disables it). A request may carry a
per-query `timeout_ms` that **lowers** the effective deadline (it can never raise
the server bound). Scan, BM25, hybrid, exact-vector, and aggregate/facet execution
poll the deadline and abandon the query with a structured `query_timeout` error
once the budget is exceeded; the session and connection remain usable. The check is
cooperative (polled on the candidate/scan loop), so the boundary is "soon after"
the deadline rather than a preemptive mid-instruction interrupt.

## Stable ranked pagination (v1.2.0)

Ranked retrieval (`text_search`, `hybrid`, `vector`) has a *total* deterministic
order — `score` descending, ties broken by the internal record id ascending —
which enables **keyset (seek) pagination** by opaque cursor token instead of
server-held offset state. `Engine::search_page(query, page_size, cursor)` returns
a page of ranked rows (with stable 1-based ranks across pages) and a `next_cursor`
token (`None` on the last page); the client passes the token back to fetch the
next page.

The token is **opaque, fixed-size, and bounded** (a 37-byte record — version,
rank offset, last `(score, id)` key, and a query fingerprint — hex-encoded to 74
chars regardless of query size) and **carries no secrets**: no query text, vector,
filter, or auth material. The embedded fingerprint binds a token to the exact
query that produced it, so a token replayed against a different query is rejected
with `invalid_request`; a malformed token is likewise rejected.

Pages are duplicate-free and gap-free when the score of already-paged rows is
stable between calls. Exact-vector similarity is corpus-independent, so vector
cursors stay duplicate-free even across concurrent writes. BM25 scores depend on
corpus statistics, so an insert/delete between pages re-scores the corpus — for
duplicate-stable BM25/hybrid paging across writes, page inside a transaction
snapshot, which fixes the corpus and the order (the `search_page` request honors
the frame's transaction id). The mechanism is exposed over AWP as the additive
`search_page` read request — `{find, page_size, cursor?}` returning
`{rows, next_cursor?}` — and advertised as the `ranked_pagination` capability. A
typed connector pagination helper is a planned follow-up; the request can be
issued directly today.

### Filters

```json
{"type": "and", "filters": [ ... ]}
{"type": "or",  "filters": [ ... ]}
{"type": "not", "filter": { ... }}
{"type": "compare", "field": "metadata.status", "op": "eq", "value": "published"}
{"type": "contains", "field": "title", "substring": "refund"}
{"type": "contains_text", "field": "body", "query": "refund policy"}
{"type": "exists", "field": "owner"}
```

`op` is one of `eq, ne, lt, lte, gt, gte, in`. Field names may be dotted document
paths. Numeric comparisons coerce int/float; non-comparable pairs do not match.

`contains` is exact substring matching. `contains_text` is tokenized full-text
matching: a record matches when it contains every distinct query token (boolean
AND), ranked by summed term frequency (not BM25). See [FULL_TEXT.md](FULL_TEXT.md).

### Vector clause

```json
{"field": "embedding", "query": [ ... ], "k": 10, "metric": "cosine"}
```

`metric` is `cosine`, `euclidean`, or `dot_product`. Results are ordered by
similarity (highest first) and each row carries a `score`.

## Planner

Read queries route through a **cost-based planner** (`auradb-query`: `plan.rs`,
`planner.rs`, `stats.rs`) that builds a **plan tree** and chooses an access path
by estimated cost before execution. Plan node types are `PointLookup`,
`IndexLookup`, `DocumentPathIndexLookup`, `FullTextIndexLookup`, `VectorSearch`,
`Scan`, `Filter`, `Sort`, `Offset`, `Limit`, `Projection`, `RelationshipInclude`,
`Cursor`, `Count`, `Exists`, and `Mutation`.

### Cost and selectivity

Cost is a candidate-row estimate derived from the collection's row count and
per-field cardinality, both read from persisted statistics. The selectivity of an
equality on a field is `rows / distinct` (its cardinality). The planner picks the
**most selective applicable index** among the candidates — a primary-key point
lookup, a secondary or document-path equality index, a full-text index for a
`contains_text` filter, or the exact vector index for a vector clause — and falls
back to a **full scan** when no index applies.

Indexes map values to record ids; the executor resolves those ids through MVCC
visibility, so an index never returns an invisible version (the DataSource /
transaction view applies snapshot or latest visibility). See
[INDEXING.md](INDEXING.md).

### Statistics

Planner statistics are persisted in `planner_stats.json`. A `CollectionStats`
holds `row_count`, `field_cardinality`, `vector_count`, `text_field_docs`, and
`avg_record_size`. Row counts are kept current on every mutation; cardinality and
the rest are recomputed by `auradb stats analyze` and on compaction. Statistics
are **advisory**: a missing or corrupt file simply falls back to live estimates —
it is never an error and never affects correctness. `auradb stats show` prints the
persisted statistics. See [CLI.md](CLI.md).

## Execution

1. **Candidate selection** - the chosen access-path node seeds candidates: a
   point/index lookup, a full-text or document-path index lookup, the exact vector
   index, or a full scan when no index applies.
2. **Filtering** - the full filter is always re-applied to candidates.
3. **Ordering** - by similarity (vector) or by `order_by` keys (stable, nulls
   last).
4. **Offset/limit**, then **materialization** (projection + relationship
   hydration + score). The planner returns ordered ids so cursors can page
   without materializing every row.

## EXPLAIN

`explain` returns the plan: collection, strategy (`vector_exact_scan`,
`index_lookup`, `full_text_scan`, `full_scan`), index used, estimated candidates,
filter presence, vector summary, ordering, includes, and warnings (e.g. large
full scans). `ExplainPlan` also carries `estimated_rows`, `estimated_cost`, and
the `plan_tree` (the chosen `PlanNode`).

- `index_lookup` with a `used_index` is reported when a secondary or
  document-path equality index seeds candidates.
- `full_text_scan` with a `used_index` is reported when a `contains_text` filter
  uses a full-text index. Without a matching index, `contains_text` honestly
  falls back to a tokenized `full_scan` with `used_index: null`.

## EXPLAIN ANALYZE

`EXPLAIN ANALYZE` executes the query and reports measured metrics alongside the
plan. The plan's `analysis` field carries an `ExplainAnalysis` with
`scanned_rows`, `matched_rows`, `returned_rows`, `execution_micros`,
`planning_micros`, and `snapshot_ts` (the snapshot timestamp when run inside a
transaction).

It is requested as an optional `"analyze": true` sibling key in the raw Query IR
sent to the existing `Explain` opcode — there is **no new opcode and no protocol
break**, so existing connectors reach it through the raw IR. See
[AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md).

## Migration estimate

`estimate_migration` compares a target schema to the current one and the data,
reporting added/removed fields, new indexes, vector indexes to build, records
that would fail validation, and whether a full scan is required. It is read-only.

## Limitations

Hybrid (text + vector) fusion ranking and BM25 full-text are not implemented.
`contains` is exact substring matching; `contains_text` is tokenized boolean-AND
matching with term-frequency ranking, not BM25.

## Transaction-scoped reads

Every read accepts an optional transaction. When a transaction id is supplied,
the query executes against the **transaction view**: the committed state
overlaid with that transaction's staged writes and deletes. The transaction
sees its own staged inserts and updates and does not see its staged deletes
(read-your-writes); the effects stay invisible to other readers until commit.

This applies uniformly across `find`, `count`, `exists`, `explain`, vector
nearest, document-path filters, full-text search, relationship `include`
hydration, and cursor paging. Index-seeded candidate selection (equality, vector,
full-text) is served from an overlay index built over the transaction view, so a
staged write is never missed and a staged delete is never returned. Correctness
is prioritized over performance: the overlay index is rebuilt per transactional
query (see [TRANSACTIONS.md](TRANSACTIONS.md)). Reads without a transaction id are
unchanged.

## EXPLAIN ANALYZE diagnostics (v0.3.1)

`EXPLAIN ANALYZE` reports measured execution alongside the plan. In addition to
the v0.3.0 fields (scanned/matched/returned rows, planning and execution time, and
the MVCC snapshot timestamp inside a transaction), v0.3.1 adds, as **additive JSON
fields**:

- `estimated_rows` — the planner's estimate, carried beside the measured actuals;
- `planner_stats_version` — the persisted statistics format version used;
- `selected_index_reason` — a short human-readable reason for the chosen access
  path (e.g. "equality lookup seeded by index `status`");
- `stale_stats` — true when statistics were absent or had no per-field cardinality,
  so the planner used default selectivity (the result is still correct; the cost
  choice may not be optimal — run `auradb stats analyze`).

All additions are additive and optional, so Aura Connector 0.3.x continues to
deserialize the v0.3.0 shape. Planner regression coverage lives in
`crates/auradb/tests/planner_regression.rs` and the ANALYZE shape in
`crates/auradb/tests/explain_analyze_fields.rs`; whatever access path the planner
chooses, the rows returned are correct.

## Ranked retrieval (v1.1.0)

The planner recognizes three ranked-retrieval clauses on a find: `vector` (exact
nearest-neighbour), `text_search` (BM25 ranked full-text), and `hybrid` (fused text+vector).
At most one may be set; results are ordered by score (descending, ties broken by record id).

- **Planner.** Ranked clauses force their access path. Candidate counts are estimated from
  full-text/vector statistics; `EXPLAIN` reports the strategy (`full_text_bm25`, `hybrid`,
  `vector_exact_scan`), the indexed field(s), ranking mode/operator, and — for hybrid — the
  fusion mode, weights, and text/vector candidate sources.
- **EXPLAIN ANALYZE.** Additionally reports scanned/matched/returned rows, per-signal
  candidate counts, and timing. The shape is additive: existing fields are unchanged.
- **Statistics.** `auradb stats analyze` refreshes the full-text document counts used to
  estimate candidates. Stale statistics never change correctness, only plan choice.

See [SEARCH_AND_RANKING.md](SEARCH_AND_RANKING.md) for the clause reference.
