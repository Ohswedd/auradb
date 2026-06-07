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

The current baseline is `v0.8.0.json`, the refreshed single-node engine baseline.
The previous baseline is `v0.6.2.json`: v0.7.0 and v0.7.1 were connector-ergonomics
releases that did **not** refresh the engine baseline, so `v0.6.2.json` is the
prior baseline to compare against. It is the single-node engine suite; multi-node
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
