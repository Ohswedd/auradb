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

## Execution

1. **Candidate selection** - if a vector clause is present, candidates come from
   the exact vector index; else if the filter contains a top-level indexed
   equality (including a document-path index on a dotted path), the index seeds
   candidates; else if a `contains_text` filter targets a field with a full-text
   index, that index seeds candidates; otherwise a full scan.
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
full scans).

- `index_lookup` with a `used_index` is reported when a secondary or
  document-path equality index seeds candidates.
- `full_text_scan` with a `used_index` is reported when a `contains_text` filter
  uses a full-text index. Without a matching index, `contains_text` honestly
  falls back to a tokenized `full_scan` with `used_index: null`.

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
