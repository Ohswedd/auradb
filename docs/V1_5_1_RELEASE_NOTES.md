# AuraDB v1.5.1 release notes

**Conformance-harness correctness — single-node production line, multi-node HA candidate
preview.**

AuraDB v1.5.1 is a patch release for conformance-harness correctness. It makes the Python
conformance harnesses safe to rerun against the same live server. There are **no** engine,
protocol, storage, query, connector, or compatibility behavior changes. AWP remains 1.
Storage format remains v2. The index snapshot format version remains 1. Aura Connector
v0.9.0 remains the tested connector line. Single-node remains production-supported.
Multi-node remains an HA candidate preview, **not** production HA. HNSW/ANN remains an
opt-in preview, **not** production ANN, with exact vector search as the correctness baseline.

The v1.5.0 tag is **not** moved; v1.5.1 is cut as a separate patch. No Aura Connector
release is needed — the connector package is unchanged.

## Why this patch

The conformance harnesses each seed fixed primary keys into named collections. Running a
harness a second time against an already-seeded server collided on those keys (the engine
and the published connector were already correct — this was a test-harness issue only):

- An `insert` of an already-present primary key raised a `unique_violation`.
- Exact `count`/result assertions drifted once a prior run's rows remained.

The published `aura-connector` 0.8.0 backward-compatibility suites were the visible
trigger: they passed on a fresh data directory but collided when rerun on an already-seeded
server.

## What changed in v1.5.1

- **Version bump** to `1.5.1` across the workspace (`Cargo.toml`, `Cargo.lock`), the CLI
  (`auradb version` / `auradb compatibility`, which continues to report `Aura Connector
  (tested): 0.9.0`), and the documentation.
- **Shared run-isolation helper** (`tests/conformance/python/_conformance_isolation.py`):
  resolves a per-run collection-name prefix and returns run-scoped subclasses of each
  harness's models. The isolation seam is the collection name — an AuraDB collection is
  addressed over the wire by its model's class name, so a per-run prefix yields a fresh,
  non-colliding keyspace. The helper relies only on the long-stable model/collection
  contract, so it works identically under the current connector and the published
  backward-compatibility connectors. It changes neither AWP, the storage format, the index
  snapshot format, nor any production server behavior.
- **Defaults just work.** With no flag the prefix is a fresh random token, so any harness
  run twice in a row passes both times — no data-dir reset and no manual cleanup. Two
  optional flags pin a namespace for reproducibility: `--run-id <token>` scopes collections
  as `<token>_<ClassName>` (with an `AURA_CONFORMANCE_RUN_ID` environment default shared
  across harnesses), and `--collection-prefix <prefix>` sets an exact literal prefix.
- **Coverage.** Every single-node connector harness (`run_connector_smoke.py`,
  `run_connector_conformance.py`, `run_connector_search.py`, `run_connector_facets.py`,
  `run_connector_group_by.py`, `run_connector_pagination.py`,
  `run_connector_query_profile.py`, `run_connector_cursor_resume.py`,
  `run_connector_timeouts.py`, `run_connector_ann_preview.py`,
  `run_connector_analyzers.py`, `run_connector_snippets.py`) plus the standard-library
  `run_conformance.py` carry run isolation. The cluster harnesses seed through idempotent
  `upsert`/count-guarded writes against a dedicated preview cluster fixture and already
  repeat safely.
- **Documentation** (`docs/CONFORMANCE.md`, `docs/TESTING.md`, `docs/RELEASE.md`): document
  run isolation, the new flags, the default repeated-run safety, and the repeated-run
  conformance gate.

## Compatibility

- **Storage format unchanged** (manifest `format_version` 2). Data directories from every
  prior v0.3.x–v1.5.x release open in place; no migration is required.
- **Aura Wire Protocol unchanged** (AWP 1). No new request or response shapes.
- **Index snapshot format unchanged** (version 1).
- **Aura Connector v0.9.0 paired** (and compatible 0.9.x). Connector 0.8.x/0.7.x/0.6.x/0.5.x
  remain supported for the existing feature set. **No connector release is required for
  v1.5.1.**
- **Configuration is backward compatible.** No new config fields; no config flag changes
  meaning.
- All v1.5.0 behavior is preserved byte-for-byte.

The frozen v1 compatibility surfaces are unchanged: **Aura Wire Protocol 1** is the stable
v1 wire protocol and **storage format v2** is the stable v1 single-node storage format, each
preserved across v1.x unless a security, correctness, safety, or corruption issue requires a
documented change or migration. See [`COMPATIBILITY.md`](COMPATIBILITY.md),
[`PROTOCOL.md`](PROTOCOL.md), [`STORAGE_ENGINE.md`](STORAGE_ENGINE.md), and
[`AURA_CONNECTOR_COMPATIBILITY.md`](AURA_CONNECTOR_COMPATIBILITY.md).

## Validation

- **Repeated-run conformance.** Against a freshly built v1.5.x server, every single-node
  harness passed twice in a row under the current `aura-connector` 0.9.0.
- **Backward compatibility.** The published `aura-connector` 0.8.0 backward-compatibility
  set passed twice in a row against the same server. The analyzer/snippet harnesses require
  a ≥0.9.0 connector (they exercise v1.5 live-analyzer/snippet features) and are not part of
  the 0.8.0 set.
- **Workspace gates.** `cargo fmt`, `clippy`, the full `cargo test` workspace, `build`,
  bench compilation, `cargo audit`, and `cargo deny` all pass. The single-node production
  drills pass (8 drills, 0 failures), and the `search eval` hybrid-keyword and
  `compare-analyzers` legs run clean.

## What did not change

- No engine, query, storage, replication, or protocol behavior change.
- No public connector API change, and no new Aura Connector release. The connector package
  is untouched; `DEFAULT_ISOLATION` stays `snapshot` and `serializable` remains a deprecated
  alias for `snapshot`.
- No production HA claim and no production ANN claim. Exact vector search remains the default
  and the correctness baseline.

## Known limitations

Honest limitations carried by this release (unchanged scope boundaries):

- **Multi-node is an HA candidate preview, not production HA.** No production automatic
  failover, no linearizable follower reads (follower reads/search are eventually
  consistent), no distributed transactions, and no dynamic membership, sharding, or
  multi-region. Single-node remains the recommended production mode.
- **Approximate (HNSW) vector search is an opt-in preview, not production ANN.** The graph is
  in-memory and rebuilt from the exact vectors (never persisted; not incremental). Exact
  vector search remains the default and the correctness baseline.
- **Re-running a harness with the same pinned `--run-id`/`--collection-prefix` reuses the
  same collections** and so can collide on fixed keys, exactly as a single shared namespace
  would; omit the flags, or vary the token, for independent runs.
