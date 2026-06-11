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
in-memory/rebuilt, re-ranked by exact similarity, and **not production ANN**. v1.3.0 adds
durable lifecycle metadata for the preview (the graph itself is still never persisted), an
`ann_fallback` policy (`exact` default / `error`) for when the preview is unavailable, the
`ANN_PREVIEW_MIN_VECTORS = 16` minimum-dataset threshold, a `vector_mode` field in EXPLAIN
(`exact`, `ann_preview`, `exact_fallback`), and the `auradb vector eval` recall/latency
harness. See [VECTORS.md](VECTORS.md).

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

## GROUP BY aggregations (v1.3.0)

The `aggregate` request's matched set — a filtered scan, or, with a `text_search` clause, the
BM25 candidate set (a *search facet*) — can be bucketed by a single scalar field with the
additive `group_by` clause, with per-group `count`, `min`, `max`, and `avg`. Because grouping
rides the same matched set as facets and metrics, it inherits **search-candidate scoping** for
free: `group_by` over a `text_search` aggregate buckets only the BM25 candidates. Groups are
ordered by descending count then ascending key, truncated to `group_limit` (default 1000) with
the full `group_count_total` reported; null/missing group keys are excluded. See
[QUERY_ENGINE.md](QUERY_ENGINE.md) for the full semantics.

## EXPLAIN and EXPLAIN ANALYZE

`EXPLAIN` reports the chosen strategy (`full_text_bm25`, `hybrid`, `vector_exact_scan`), the
indexed field(s), ranking mode and operator, and for hybrid the fusion mode, weights, and
candidate sources. For a vector query it also reports the resolved `vector_mode` (`exact`,
`ann_preview`, or `exact_fallback`). `EXPLAIN ANALYZE` additionally reports
scanned/matched/returned rows, candidate counts per signal, and timing, and (v1.3.0) the
additive query-profile fields `plan_id`, `deadline_ms`, and `timeout_checked`. See
[QUERY_ENGINE.md](QUERY_ENGINE.md).

## Relevance evaluation and tuning (v1.4.0)

`auradb search eval` measures ranked-retrieval relevance against a committed
**relevance dataset** so a release gate can catch ranking regressions. It is
offline, deterministic, needs no Docker and no external embeddings, and uses only
the retrieval paths above — it adds no new ranking behaviour.

```bash
# The corpus is ingested into a fresh data directory, so point --data-dir at a
# throwaway path that does not already hold the RelevanceDoc collection.
rm -rf .local/search-eval
auradb search eval \
  --data-dir .local/search-eval \
  --corpus fixtures/relevance/small_corpus.jsonl \
  --queries fixtures/relevance/small_queries.jsonl \
  --qrels fixtures/relevance/small_qrels.jsonl \
  --mode bm25 --k 10 --json
```

The dataset format (JSONL corpus, queries, and graded qrels) is documented in
[`fixtures/relevance/README.md`](../fixtures/relevance/README.md). The command
emits a machine-readable JSON report with **MRR@k**, **NDCG@k**, and **Recall@k**
both aggregated and per query, plus the ids each query returned and honest
warnings (queries without judgments, missing query vectors, qrels referencing
unknown ids). A malformed dataset exits non-zero.

**These metrics are fixture-specific** — a regression signal for *this* dataset,
never a universal benchmark or a relevance guarantee on other corpora.

Modes:

- **`bm25`** — ranked full-text search over the concatenated text field.
- **`vector_exact`** — exact cosine nearest-neighbour search (the correctness
  baseline; never the approximate preview, so this makes no production-ANN claim).
- **`hybrid`** — BM25 + exact vector fused by `weighted_sum`.

**BM25 `k1`/`b` tuning.** The `--k1` and `--b` flags map directly to the
already-existing `text_search` overrides (defaults `k1 = 1.2`, `b = 0.75`); when
omitted the report echoes the engine defaults under `preset: "default"`. To tune,
sweep presets and compare the measured metrics:

```bash
for k1 in 0.9 1.2 1.6; do for b in 0.50 0.75 1.0; do
  rm -rf .local/se && auradb search eval --data-dir .local/se \
    --corpus fixtures/relevance/small_corpus.jsonl \
    --queries fixtures/relevance/small_queries.jsonl \
    --qrels fixtures/relevance/small_qrels.jsonl \
    --mode bm25 --k 10 --k1 "$k1" --b "$b" --json | \
    jq '{k1: .bm25.k1, b: .bm25.b, ndcg: .metrics.ndcg_at_k}'
done; done
```

Pick the preset that maximizes NDCG@k **on your own dataset**: the guidance is
*measured*, not guaranteed, and the best `k1`/`b` are corpus-dependent.

**Hybrid weight calibration.** `--text-weight` / `--vector-weight` set the fusion
weights (defaults `0.7` / `0.3`); the report echoes them back. Sweep them the same
way and compare NDCG@k to calibrate the text/vector balance for your data. Exact
vector remains the baseline and ANN remains a preview throughout.

## Analyzers and tokenization (v1.5.0)

AuraDB v1.5.0 adds a small, deterministic **analyzer framework** that decides how
text is split into tokens. An analyzer is selected by name; the built-in presets
are:

| Preset | Behavior |
|--------|----------|
| `default` | The v1.x tokenizer: lowercase, split on every non-alphanumeric boundary. **Selecting it changes nothing** — it is the current behavior, given a name. |
| `simple`  | The same tokenization as `default`. On any input it emits the same terms; documented as equal to `default`. |
| `ascii_fold` | `simple` plus a fixed ASCII-folding table for common Latin diacritics (for example `café` → `cafe`, `naïve` → `naive`). No external dictionary; non-Latin scripts pass through unchanged. |
| `keyword` | The whole field collapses to a single normalized term for **exact whole-field matching** of short fields. A partial-term query will not match a multi-word field. |
| `english_basic` | `simple` plus a small built-in English stopword list and a conservative plural fold (`backups` → `backup`, `boxes` → `box`, `policies` → `policy`, `queries` → `query`). A tiny built-in helper, **not** a stemmer and **not** full language-aware NLP: no dictionary, no `-ing`/`-ed` handling, no part-of-speech model. The fold protects common false positives — words ending in `ss`/`us`/`is`/`ns` are never stripped, so singulars like `class`, `status`, `analysis`, and `lens` are left intact (`lens` stays `lens`). It is applied symmetrically to query and index. |

Guarantees and non-goals:

- **`default` preserves current search semantics exactly.** Omitting the analyzer,
  or passing `--analyzer default` (or sending no analyzer over the wire), is
  byte-identical to v1.4.
- Tokenization is **deterministic**: a given preset and input always produce the
  same token order, token text, and byte offsets.
- Every preset except `english_basic` applies **no stemming, no stopword list, and
  no language model**; `english_basic` removes a small fixed stopword list and folds
  a narrow, tested set of plural suffixes. No preset uses a language model or
  external dictionary. The analyzers make no language claims beyond the mechanical
  transformations above.
- Token **byte offsets** index the original (pre-folding) text, so they drive
  highlighting directly.
- **Offline `search eval` and live search share the same analyzer code.** A preset
  behaves identically whether selected in the eval harness (which analyzes the
  corpus text up front) or on a live query (which evaluates the analyzer over the
  persisted default-tokenized index — see below).

### Analyzers in `search eval`

`auradb search eval --analyzer <name>` applies the analyzer **symmetrically** to
the corpus text and the query text, so a non-default analyzer is a consistent
evaluation rather than a query-only mismatch. An unknown analyzer name is a loud,
structured error — there is no silent fallback. The report records the effective
analyzer under the `analyzer` field.

```bash
rm -rf .local/se-fold
auradb search eval \
  --data-dir .local/se-fold \
  --corpus fixtures/relevance/analyzer_corpus.jsonl \
  --queries fixtures/relevance/analyzer_queries.jsonl \
  --qrels fixtures/relevance/analyzer_qrels.jsonl \
  --mode bm25 --analyzer ascii_fold --k 10 --json
```

To compare several analyzers over one dataset in a single run:

```bash
rm -rf .local/se-compare
auradb search eval compare-analyzers \
  --data-dir .local/se-compare \
  --corpus fixtures/relevance/analyzer_corpus.jsonl \
  --queries fixtures/relevance/analyzer_queries.jsonl \
  --qrels fixtures/relevance/analyzer_qrels.jsonl \
  --mode bm25 --analyzers default,simple,ascii_fold,keyword,english_basic --k 10 --json
```

As with all relevance metrics, the per-analyzer numbers are **fixture-specific**
regression signals, not a universal benchmark. The analyzer affects the
text-bearing modes (`bm25`, `hybrid`); for `vector_exact` it is recorded but has
no effect.

### Analyzers on live search (over AWP)

A ranked `text_search` (and the text side of a `hybrid`) clause carries an optional
`analyzer` field. It is **additive and defaulted**: an omitted or `default` analyzer
is byte-identical to v1.4, so existing clients and stored queries are unaffected,
and the AWP version is unchanged. A v1.5.0 server advertises the `query_analyzers`
capability; the connector gates a non-default analyzer on it and otherwise raises a
capability error rather than silently dropping the request.

How a non-default analyzer matches the existing index without re-indexing or
changing the storage / index-snapshot format:

- `default` / `simple` use the existing default-tokenized search verbatim.
- `ascii_fold` and `english_basic` are **per-token transforms** of the default
  tokenizer, so the engine evaluates them over a transformed view of the persisted
  default postings at query time — symmetric with how `search eval` analyzes the
  corpus up front.
- `keyword` is whole-field: the engine gathers candidates from the default index
  and confirms each against the stored field text. The **hybrid** text signal uses
  this same whole-field keyword path — `keyword` is accepted in `hybrid` search, its
  text component contributes the exact whole-field matches, and the vector component
  is fused as usual. There is no silent fallback: `EXPLAIN` reports the hybrid text
  source as `keyword:<field>` (rather than `bm25:<field>`) when `keyword` is selected.

`EXPLAIN` / `EXPLAIN ANALYZE` report the effective analyzer under the ranked-text
(or hybrid) plan's `analyzer` field.

## Highlight / snippet support (v1.5.0)

A ranked text (or hybrid) search can request **opt-in** plain-text snippets with
highlight ranges. The request is an additive field on the find request:

```json
{ "collection": "Doc",
  "text_search": { "field": "body", "query": "restore backup" },
  "snippet": { "fields": ["body"], "max_fragments": 2, "fragment_chars": 200 } }
```

Each result row gains an additive `snippets` array (omitted entirely when no
snippet was requested, so existing clients are unaffected). The deterministic
builder (`auradb_query::snippet`) enforces:

- a **field allowlist** — a snippet is only ever built for a field named in
  `snippet.fields`; a field absent from that list (including any internal,
  `_`-prefixed name) is never read or returned, so internal or unrequested fields
  cannot leak;
- **server-clamped caps** — fragment count and fragment length are clamped to server
  maximums, so a snippet can never echo an entire large document regardless of what
  the client asks for;
- **safe skipping** — a requested field that is absent or non-textual is skipped, not
  a panic and not an empty placeholder;
- **deterministic** output with offsets that land on character boundaries (verified
  for multibyte/Unicode text).

Snippet output is **plain text** — fragments carry the original characters plus byte
ranges (offsets into the fragment text) marking matches. Callers that render to HTML
must escape the text themselves; the builder makes no markup claims.

Snippets are gated by the `search_snippets` capability: a v1.5.0 server advertises
it, and a snippet request against a server that does not advertise it is refused with
a capability error rather than silently ignored. Opt-in only: a query without a
`snippet` clause returns no snippets and behaves exactly as before.

## Operations

- Full-text and vector indexes survive restart, backup/restore, and compaction; BM25 length
  statistics persist in the index snapshot and rebuild safely if missing.
- `auradb index check` validates BM25 and vector index statistics.
- `auradb stats analyze` refreshes the full-text statistics the planner uses.
- `auradb search explain --input query.json [--analyze]` inspects a ranked query's plan.
- `auradb search eval` measures fixture-specific MRR@k/NDCG@k/Recall@k for BM25, exact-vector,
  and hybrid retrieval (see "Relevance evaluation and tuning" above).
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
