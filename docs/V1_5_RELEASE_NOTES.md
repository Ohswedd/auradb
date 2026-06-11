# AuraDB v1.5.0 release notes

**Theme: live search analyzers, snippets, and search-quality expansion.**

AuraDB v1.5.0 is a minor release on the v1 single-node production line. It takes the
search-quality work live over the wire: query-time analyzer selection and opt-in
snippets/highlights, both negotiated additively through new server capabilities. It
introduces **no new wire framing, storage format, or index snapshot format** — the
`default` analyzer reproduces v1.x tokenization and ranking exactly, and clients that do
not negotiate the new capabilities see unchanged behavior.

AuraDB v1.5.0 is paired with **Aura Connector v0.9.0** (compatible 0.9.x). Connector
v0.8.x/0.7.x/0.6.x/0.5.x remain supported for the existing feature set; because there is
no wire change, an older connector remains fully compatible with AuraDB 1.5.0 — it simply
does not request analyzer or snippet fields.

See [COMPATIBILITY.md](COMPATIBILITY.md), [AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md),
[SEARCH_AND_RANKING.md](SEARCH_AND_RANKING.md), [CONFORMANCE.md](CONFORMANCE.md), and
[TESTING.md](TESTING.md).

## Shipped

Analyzers:

- **Deterministic analyzer framework** in `auradb-index` — pluggable, deterministic
  analyzers with no external NLP dependencies.
- **Live query-time analyzer selection** — ranked text search and hybrid search accept a
  per-query analyzer over the wire.
- **`query_analyzers` server capability** — negotiates live analyzer selection additively.
- **Analyzer presets**: `default`, `simple`, `ascii_fold`, `keyword`, and `english_basic`.
- **`english_basic`** — a small fixed stopword list and conservative plural folding.
- **`keyword` analyzer** support in both text search and hybrid search (indexes the whole
  field as a single token).
- **Analyzer-aware EXPLAIN/profile output** — the selected analyzer is reflected in
  query-profile fields.

Snippets:

- **Live opt-in snippets/highlights** — ranked text search can return plain-text snippets
  with highlight ranges.
- **`search_snippets` server capability** — negotiates opt-in snippets additively.
- **Snippet field allowlist, fragment caps, plain-text output, and Unicode-safe ranges** —
  snippets are returned only for explicitly requested fields, with character-boundary
  (not byte-boundary) highlight ranges.

Search evaluation:

- **Search-eval analyzer support** — `auradb search eval --analyzer <name>`.
- **`compare-analyzers`** — evaluates a set of analyzers over the same fixture in one run.
- **Analyzer relevance fixtures** — `fixtures/relevance/analyzer_*.jsonl`.

Conformance:

- **Live analyzer conformance suite** (`tests/conformance/python/run_connector_analyzers.py`).
- **Live snippet conformance suite** (`tests/conformance/python/run_connector_snippets.py`).

## Unchanged

- **AWP 1** — Aura Wire Protocol 1, frozen for the v1 line.
- **Storage format v2** — no on-disk change.
- **Index snapshot format version 1** — no index snapshot change.
- **Single-node is production-supported.**
- **Multi-node clustering is an HA candidate preview only**, not production HA.
- **Approximate (HNSW) vector search is an opt-in preview only**, not production ANN.
- **Exact vector search is the default and correctness baseline.**
- **Query timeouts remain cooperative.**

## Known limitations

- **`english_basic` is deterministic and small.** It is not full NLP and not a full
  stemmer — it applies a fixed stopword list and conservative plural folding only.
- **Relevance scores are fixture-specific regression signals, not universal benchmarks.**
  Analyzer comparison metrics are computed over small committed fixtures for regression
  signal and tuning guidance, not a cross-corpus quality guarantee.
- **Snippets are plain text** and are only returned for explicitly requested fields,
  subject to the field allowlist and fragment caps.
- **Multi-node remains an HA candidate preview only, not production HA.**
- **ANN/HNSW remains an opt-in preview only, not production ANN.**
- **The HNSW graph is rebuilt in memory and not persisted.**
