# Relevance fixtures

Small, committed, synthetic datasets for the `auradb search eval` relevance
harness. They are **regression fixtures**, not universal benchmarks: the metrics
they produce describe how AuraDB's ranked retrieval orders *these* documents for
*these* queries. They say nothing about search quality on any other corpus.

Everything here is original, synthetic text written for this repository. There
are no external downloads, no copyrighted material, and no private or
proprietary text. Keep it that way (see "Adding a dataset" below).

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `small_corpus.jsonl`  | one document per line | the searchable documents |
| `small_queries.jsonl` | one query per line    | the evaluation queries |
| `small_qrels.jsonl`   | one judgment per line | graded relevance judgments |

All three are [JSON Lines](https://jsonlines.org/): one JSON object per line, no
enclosing array. IDs are stable; do not renumber existing rows.

## Schemas

### Corpus (`*_corpus.jsonl`)

```json
{"id":"doc-001","title":"Backup restore rehearsal","body":"How to create, verify, and restore an AuraDB backup into a fresh data directory.","tags":["backup","restore","operations"],"category":"ops","vector":[0.90,0.10,0.00,0.20]}
```

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | string | yes | stable, unique document id |
| `title` | string | no | included in the searchable text |
| `body` | string | no | included in the searchable text |
| `tags` | string[] | no | included in the searchable text |
| `category` | string | no | included in the searchable text |
| `vector` | number[] | no | exact-vector embedding; all docs must share one length, or omit it everywhere |

The harness concatenates `title`, `body`, `tags`, and `category` (space
separated) into a single full-text-indexed field for BM25 — so a query term
matches whether it appears in the title, body, tags, or category. Exact text
relevance works with no `vector` field at all; vectors are only needed for the
`vector_exact` and `hybrid` modes.

### Queries (`*_queries.jsonl`)

```json
{"id":"q-001","text":"restore backup into fresh data directory","tags":["backup","restore"],"vector":[0.90,0.05,0.00,0.20]}
```

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | string | yes | stable, unique query id |
| `text` | string | yes | the query text used for BM25 and hybrid |
| `tags` | string[] | no | informational only |
| `vector` | number[] | no | query embedding for `vector_exact` / `hybrid` |

### Judgments (`*_qrels.jsonl`)

```json
{"query_id":"q-001","doc_id":"doc-001","relevance":3}
```

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `query_id` | string | yes | must reference a query id |
| `doc_id` | string | yes | must reference a document id |
| `relevance` | integer | yes | graded relevance, `>= 0` (we use `0..=3`) |

Grade convention: `0` not relevant, `1` marginally relevant, `2` relevant,
`3` highly relevant. A judgment whose `relevance` is `>= 1` counts as a relevant
hit for the binary metrics (MRR, Recall). NDCG uses the graded values directly.
Qrels that reference an unknown query or document id are ignored with a warning
in the report rather than failing the run.

## Scoring metrics

The harness reports three standard ranked-retrieval metrics at a cutoff `k`,
each in `[0, 1]`, both per query and averaged across queries with at least one
relevant judgment:

* **MRR@k** — reciprocal of the rank of the first relevant document (0 if none in
  the top `k`).
* **NDCG@k** — discounted cumulative gain (exponential gain `2^grade - 1`,
  `log2(rank + 1)` discount) normalized by the ideal ordering.
* **Recall@k** — fraction of all relevant documents found within the top `k`.

These numbers are **fixture-specific**. They are a regression signal: a change
that moves them is worth understanding before release. They are not a claim about
AuraDB's relevance on any other dataset.

## Running

```bash
# Use a throwaway data directory — the harness ingests the corpus and the
# directory must not already contain the RelevanceDoc collection.
rm -rf .local/search-eval
auradb search eval \
  --data-dir .local/search-eval \
  --corpus fixtures/relevance/small_corpus.jsonl \
  --queries fixtures/relevance/small_queries.jsonl \
  --qrels fixtures/relevance/small_qrels.jsonl \
  --mode bm25 --k 10 --json
```

Modes: `bm25` (text only), `vector_exact` (exact cosine vectors — the
correctness baseline, never the approximate preview), and `hybrid` (BM25 + exact
vector fused). See `docs/SEARCH_AND_RANKING.md` for BM25 `k1`/`b` tuning and
hybrid weight calibration.

## Adding a dataset

1. Write original, synthetic text. **Never** add copyrighted, scraped, private,
   or proprietary content — these fixtures are committed to a public repository.
2. Keep it small enough to commit and to run in CI in well under a second.
3. Use a new prefix (e.g. `ops_corpus.jsonl`, `ops_queries.jsonl`,
   `ops_qrels.jsonl`). The dataset label in the report is derived from the corpus
   file name with a trailing `_corpus` stripped.
4. Use stable, unique ids; never renumber existing rows (it breaks the
   regression baseline).
5. If you add vectors, give every document a vector of the same length, and add
   matching query vectors.
6. Run all three modes and record the expected metric bands in the regression
   test so a drift is caught.
