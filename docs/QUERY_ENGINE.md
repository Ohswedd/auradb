# Query Engine

`auradb-query` defines the Query IR and executes it over a `DataSource` (the
engine).

## Query IR

The IR is a transparent JSON model (the documented Aura-Connector compatibility
layer). Reads use `ReadRequest`:

- `Find(FindQuery)` - `collection`, optional `filter`, optional `vector`,
  `order_by`, `limit`, `offset`, `projection`, `includes`.
- `Count(CountQuery)` and `Exists(ExistsQuery)` - `collection` + optional filter.

Writes use `Mutation`: `Insert`, `BulkInsert`, `Update`, `Delete`, `Upsert`.

### Filters

```json
{"type": "and", "filters": [ ... ]}
{"type": "or",  "filters": [ ... ]}
{"type": "not", "filter": { ... }}
{"type": "compare", "field": "metadata.status", "op": "eq", "value": "published"}
{"type": "contains", "field": "title", "substring": "refund"}
{"type": "exists", "field": "owner"}
```

`op` is one of `eq, ne, lt, lte, gt, gte, in`. Field names may be dotted document
paths. Numeric comparisons coerce int/float; non-comparable pairs do not match.

### Vector clause

```json
{"field": "embedding", "query": [ ... ], "k": 10, "metric": "cosine"}
```

`metric` is `cosine`, `euclidean`, or `dot_product`. Results are ordered by
similarity (highest first) and each row carries a `score`.

## Execution

1. **Candidate selection** - if a vector clause is present, candidates come from
   the exact vector index; else if the filter contains a top-level indexed
   equality, the index seeds candidates; otherwise a full scan.
2. **Filtering** - the full filter is always re-applied to candidates.
3. **Ordering** - by similarity (vector) or by `order_by` keys (stable, nulls
   last).
4. **Offset/limit**, then **materialization** (projection + relationship
   hydration + score). The planner returns ordered ids so cursors can page
   without materializing every row.

## EXPLAIN

`explain` returns the plan: collection, strategy (`vector_exact_scan`,
`index_lookup`, `full_scan`), index used, estimated candidates, filter presence,
vector summary, ordering, includes, and warnings (e.g. large full scans).

## Migration estimate

`estimate_migration` compares a target schema to the current one and the data,
reporting added/removed fields, new indexes, vector indexes to build, records
that would fail validation, and whether a full scan is required. It is read-only.

## Limitations

Hybrid (text + vector) fusion ranking and BM25 full-text are not implemented;
`contains` is exact substring matching. Within an open transaction, filtered
finds reflect the committed snapshot (point `txn_get` honors staged writes).
