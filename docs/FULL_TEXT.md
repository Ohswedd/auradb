# Full-text search

AuraDB 0.2.0 provides a basic, honest full-text search facility: a tokenized
inverted index with boolean-AND matching and term-frequency ranking. It is
deliberately simple, and it is **not** BM25 and not a hybrid text/vector ranker.

## Declaring a full-text index

Declare a full-text index in a schema via the `indexes` array. The target must
be a string field.

```json
{
  "indexes": [
    { "path": "body", "kind": "full_text" }
  ]
}
```

The index is maintained on insert, update, and delete, and it is persisted to
disk and restored across restart (see [INDEXING.md](INDEXING.md) and
[STORAGE_ENGINE.md](STORAGE_ENGINE.md)).

## Tokenizer

The tokenizer is intentionally minimal:

- **Case folding.** Text is lowercased before tokenizing.
- **Boundaries.** Text is split on every non-alphanumeric boundary (punctuation
  and whitespace), so `"Refund-Policy v2"` yields `refund`, `policy`, `v2`.

### Stop-word behavior

No stop-word removal is applied. Every token is indexed, including common words
such as `the`, `a`, and `of`. This is a deliberate choice and is documented here
so query results are predictable.

## Querying with `contains_text`

Query a full-text field with a `contains_text` filter:

```json
{ "type": "contains_text", "field": "body", "query": "refund policy" }
```

### Matching semantics (boolean AND)

The query string is tokenized with the same tokenizer. A record matches when it
contains **every distinct query token**. In the example above, a record must
contain both `refund` and `policy` to match.

### Ranking

Matching records are ranked by **simple summed term frequency** (the total number
of occurrences of the query tokens in the record), highest first. This is a
plain term-frequency score. It is **not** BM25: there is no inverse document
frequency, no length normalization, and no field weighting.

## EXPLAIN

When a full-text index exists on the queried field, `EXPLAIN` reports:

```json
{ "strategy": "full_text_scan", "used_index": "body" }
```

When no full-text index exists on the field, `contains_text` honestly falls back
to a tokenized full scan, and `EXPLAIN` reports:

```json
{ "strategy": "full_scan", "used_index": null }
```

The fallback applies the same tokenization and boolean-AND semantics, so results
are identical to the indexed path; only the access method differs.

## Limitations

- Term-frequency ranking only; not BM25, no IDF, no length normalization.
- Boolean AND across distinct query tokens; there is no phrase, proximity,
  prefix, wildcard, or fuzzy matching.
- No stemming, lemmatization, or stop-word removal.
- No hybrid (text + vector) fusion ranking.

These are tracked in the [roadmap](ROADMAP.md).
