# Benchmark baselines

Each `vX.Y.Z.json` file is a snapshot produced by:

```bash
cargo run -p auradb-cli -- bench --json --output benches/baseline/vX.Y.Z.json
```

The report records the AuraDB version, the record count, the command, the source
commit (when git is available), the machine (OS, architecture, logical CPUs),
and one measurement per category with its unit (`ops_per_sec`, `ns_per_op`, or
`seconds`).

All values are measured live; none are hand-written. Benchmarks are
hardware-dependent and are meant to detect regressions when compared against a
snapshot taken on the **same** machine, not as universal performance claims. A
number from one laptop is not comparable to a number from another.

The benchmark opens the engine with `sync_on_commit = false` so it measures
engine work rather than disk-flush latency. See [docs/BENCHMARKS.md](../../docs/BENCHMARKS.md).

The current baseline is `v1.2.0.json`, captured on the v1.2.0 query-ergonomics
release branch with a **release** build. v1.2.1 is a conformance and documentation
hardening release with no engine or performance changes, so it carries the v1.2.0
baseline forward unchanged (no new baseline file is added). v1.2.0 adds four
measurements —
`aggregate_count`, `facet_terms`, `vector_ann_preview` (the opt-in approximate HNSW
preview, next to `vector_exact_nearest`), and `ranked_pagination_first_page` —
alongside the existing suite. Note the approximate preview is **slower than exact at
this small scale** (graph-traversal overhead; HNSW's sub-linear scaling only pays off
at large vector counts) — see [docs/BENCHMARKS.md](../../docs/BENCHMARKS.md). Always
capture with a release build (a debug build is many times slower and not comparable),
and compare only files from the **same machine**: the v1.1.0 baseline was captured on
different hardware, so cross-version absolute numbers here are not directly comparable
(`auradb bench compare` warns about this). The prior baseline is `v1.1.0.json`
(search and ranking: `full_text_bm25`, `hybrid_search`); before it `v1.0.1.json`,
`v1.0.0.json`, `v0.9.2.json`, `v0.9.1.json` and `v0.9.0.json`, and
before it `v0.8.1.json` and `v0.8.0.json`
(v0.7.0 and v0.7.1 were connector-ergonomics releases that did **not** refresh the
engine baseline). It is the single-node engine suite; multi-node
replicated-write and recovery latency (leader write, follower catch-up, snapshot
install, reconnect recovery, cluster status) is topology- and network-dependent and
is exercised — and bounded — by the cross-process preview tests
(`crates/auradb-replication/tests/multi_node.rs`) rather than committed here.

## Comparing baselines

Two comparators read these JSON files; both are **unit-aware** (`ops_per_sec` is
higher-better; `ns_per_op` and `seconds` are lower-better) and **warn by default**:

- `auradb bench compare --baseline <a> --current <b> [--fail-threshold-percent N]`.
- `scripts/compare_benchmarks.py <a> <b> [--fail-threshold-percent N]` — a
  dependency-free Python comparator.

Baselines are **hardware- and build-profile-sensitive**: compare only files
captured on the **same machine** with the **same build profile**.
