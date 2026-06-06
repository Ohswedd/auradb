# Aura Connector Conformance

AuraDB is tested against client-side scenarios that mirror Aura Connector usage,
over the real wire protocol.

## Two harnesses

1. **Rust** (`crates/auradb-conformance`) - a `Client` implementing the client
   side of AWP and a scenario suite (`run_all`). The integration test
   `crates/auradb-conformance/tests/conformance.rs` starts a real server on an
   ephemeral port and asserts every scenario passes; it also verifies data
   survives a server restart.

2. **Python** (`tests/conformance/python/run_conformance.py`) - a self-contained
   AWP client (standard library only) that runs the same scenarios against a
   running server. It demonstrates cross-language wire compatibility and accepts
   `--auth-token`, `--tls-ca`, and `--tls-server-name` to exercise authenticated
   and TLS-terminated servers over the wire.

## Scenarios

ping, health, schema create, insert, find, filter, document field, document-path
index (with an EXPLAIN check), full-text search (with an EXPLAIN check),
relationship include, vector nearest, explain, count, exists, migration
estimate, update/upsert/delete, transaction commit/rollback, and
transaction-scoped reads (a staged write is visible to the transaction's own
read but not to a non-transactional read until commit). The Rust test also
forces cursor streaming via a small page size.

### MVCC and planner scenarios (0.3.0)

- **`snapshot_isolation_later_commit_invisible`** - a transaction that pins its
  snapshot at `begin` does not observe a write another transaction commits
  afterward.
- **`write_conflict_rejected`** - committing a transaction whose write set was
  modified concurrently is rejected with a conflict (first-committer-wins).
- **`explain_analyze_shape`** - `EXPLAIN ANALYZE` (requested via the raw Query IR
  `"analyze": true` flag) returns the plan plus execution metrics
  (scanned/matched/returned rows, execution and planning time, snapshot ts).
- **`planner_uses_index`** - the cost-based planner selects an index access path
  for a selective equality rather than a full scan.

These run as part of the conformance suite alongside the scenarios above.

### Cluster scenarios (0.4.0)

These scenarios exercise single-node cluster mode end to end. They confirm the
cluster path works without changing the non-cluster guarantees:

- **`single_node_cluster_connect`** - a server started with `[cluster] enabled =
  true` and no peers accepts connections and serves requests normally.
- **`cluster_status_and_capability`** - the health report includes the additive
  `cluster` section (node id, cluster id, role `leader`, term, commit/applied
  indices, `single_node = true`, replication lag) and the wire protocol version is
  unchanged.
- **`leader_accepts_writes`** - the single node is the leader and accepts writes;
  there is no `not_leader` rejection in single-node mode.
- **`raft_backed_write_survives_restart`** - a write committed through the Raft log
  is present after the server restarts (committed-but-unapplied entries replay).
- **`snapshot_create_restore`** - a snapshot captures schemas and current records
  and restores them into a fresh engine with identical visible state.
- **`non_cluster_mode_unchanged`** - with cluster mode disabled (the default),
  every scenario above and every prior scenario still passes, confirming the
  default path is unchanged.

These run as part of the conformance suite. See [CLUSTERING.md](CLUSTERING.md) and
[REPLICATION.md](REPLICATION.md).

### Three-node preview scenarios (0.5.0)

> **AuraDB v0.5.0 introduces a controlled, experimental multi-node server
> preview. Single-node mode remains the recommended production mode.**

The v0.5.0 multi-node preview is exercised across real server processes over real
TCP sockets (the loopback three-node configuration). The scenarios confirm the
cross-process cluster behaves as described:

- **Detect leader** — after the cluster elects, a leader is reported (via
  `auradb cluster leader` / the `cluster` status section).
- **Write to leader** — a write sent to the leader's client address is accepted,
  replicated, and committed on a majority.
- **Follower returns `not_leader`** — a write sent to a follower returns the
  structured `not_leader` error with a leader hint.
- **Follower health / status** — a follower stays healthy and reports per-peer
  cluster state (`preview_multi_node`, `quorum_available`, and the `peers` array)
  in `auradb status --json`.
- **Stop + restart follower catch-up** — a stopped follower, after restart,
  replays its durable log and is brought current by the leader.

The Aura Connector validates against the **leader's** client address; a write
routed to a follower surfaces `not_leader`, which a 0.3.x connector handles
additively. For v0.5.0 the published `aura-connector` 0.3.0 smoke suite was run
against the elected leader of a loopback three-node cluster (12/12 checks passed);
the auth/TLS connector matrix and full conformance suite run in `conformance.yml`.
See [CLUSTERING.md](CLUSTERING.md) and [TESTING.md](TESTING.md).

### v0.5.1 hardening

v0.5.1 keeps the above scenarios and adds coverage exercised by the cluster CI
workflow: **leader restart and re-election** (a stopped leader's term is taken
over by the surviving majority; the old leader rejoins as a follower and catches
up), **follower catch-up across 1,000+ entries**, **`not_leader` ergonomics**
(the leader hint carries the leader's client address and the wire error is marked
`retryable`, and the connection stays usable), and **peer TLS validation**
(wrong CA / wrong SAN rejected, rotated certificate accepted). The published
`aura-connector` smoke against the elected leader continues to run in CI; local
runs require PyPI access and are documented rather than faked when offline.

### v0.6.0 fail-stop recovery and snapshot install

v0.6.0 keeps every scenario above and adds **peer snapshot install** coverage: a
follower behind the leader's compacted prefix is restored by a bounded
single-message snapshot install and then resumes AppendEntries, and oversized,
wrong-cluster, bad-digest, and future-format snapshots are rejected without
touching follower state (`crates/auradb-replication/tests/multi_node.rs`).

The published **Aura Connector 0.3.0** was installed from PyPI and run locally
against a v0.6.0 server (the `auradb version` reports `0.6.0`): the AWP protocol
conformance passed **18/18**, the connector smoke **12/12**, and the full
connector conformance **15/15** — no connector changes are required and AWP stays
at v1. The wire additions in v0.6.0 (additive fail-stop diagnostics fields on the
health report's `cluster` section) are ignored by the 0.3.x connector.

### v0.6.1 snapshot install and published-cluster smoke hardening

v0.6.1 keeps every scenario above and adds larger and concurrent-write
snapshot-install coverage (data, index, planner-stats, and MVCC-timestamp
convergence; no duplicate apply under concurrent leader writes) and
snapshot-needed / follower-lag diagnostics with a live `auradb cluster doctor
--addr` (`crates/auradb-replication/tests/multi_node.rs`,
`crates/auradb-cli/tests/cluster_diagnostics.rs`). The connector leader-hint UX
review was **docs-only** (Option A): the `not_leader` leader-hint message and the
no-infinite-retry contract are pinned by
`crates/auradb-server/tests/not_leader.rs`
(`connector_not_leader_message_includes_leader_hint`, `connector_no_infinite_retry`).

For v0.6.1, local validation used the stdlib AWP harness
(`tests/conformance/python/run_conformance.py`, 18/18 against a v0.6.1 server
whose `auradb version` reports `0.6.1`) and the Rust conformance crate
(`auradb-conformance`). Published **Aura Connector 0.3.0** conformance is covered
by CI (`conformance.yml`) and must pass before release — no connector changes are
required and AWP stays at v1. The additive v0.6.1 snapshot/lag diagnostics fields
on the health report's `cluster` section and per-peer status are ignored by the
0.3.x connector.

## Running

Rust (no server needed - the test spawns one):

```bash
cargo test -p auradb-conformance
```

Python (against a running server):

```bash
cargo run --release -p auradb-cli -- server --data-dir .local/auradb --port 7171 &
python tests/conformance/python/run_conformance.py --addr 127.0.0.1:7171
```

Python against an authenticated, TLS-terminated server:

```bash
python tests/conformance/python/run_conformance.py --addr 127.0.0.1:7171 \
  --auth-token "your-secret" --tls-ca .local/certs/ca.crt
```

## Official-client harnesses

Two Python harnesses drive a running server through the published Aura Connector
and its native AuraDB backend:

- `tests/conformance/python/run_connector_smoke.py` - a minimal, fast scenario
  (connect, ping, auth, TLS, schema, insert, find, stream, read-your-writes
  transaction, vector nearest, full-text, document path, close).
- `tests/conformance/python/run_connector_conformance.py` - the full scenario
  suite.

Both accept `--auth-token`, `--tls-ca`, and `--tls-server-name`.

## CI

The `conformance.yml` workflow runs the standard-library Python harness against a
live server in three configurations (auth disabled, auth enabled with a rejection
check, and TLS), and runs the connector smoke (auth disabled and auth plus TLS)
and the full connector conformance against a freshly built server with the
published connector installed.

## Status

All Rust scenarios pass in CI via `cargo test`. The standard-library Python
harness passes all scenarios against a locally running server. The connector
harnesses were also validated locally with the published `aura-connector` 0.3.0
(installed from PyPI within `aura-connector>=0.3,<0.4`): the smoke passed in
plaintext, auth, and TLS-plus-auth modes (11/11 checks each), and the
standard-library Python wire conformance passed over TLS-plus-auth (17/17
scenarios), with no token, token hash, or private key appearing in the server
logs.

For the **v0.4.1** release the connector conformance gap was closed against the
**published** `aura-connector` 0.3.0 (installed from PyPI within
`aura-connector>=0.3,<0.4`; `aura.__version__ == "0.3.0"`), driven through its
native AuraDB backend against a freshly built v0.4.1 server in **both** supported
deployment modes:

- **Non-cluster (recommended) mode** — `run_connector_smoke.py` 12/12 checks and
  `run_conformance.py` 18/18 scenarios passed; the server's `health()` frame
  carries no `cluster` section and the connector handles it cleanly.
- **Single-node cluster mode** (`examples/auradb.cluster.local.toml`, writes
  routed through the Raft log) — `run_connector_smoke.py` 12/12 checks and
  `run_conformance.py` 18/18 scenarios passed. The additive `cluster` health
  section is present and honest (`single_node = true`, `peer_count = 0`,
  `applied_index == commit_index`, role `leader`) and the published 0.3.x
  connector ignores the unknown field without error.

`not_leader` was validated by the staged server-layer test
(`crates/auradb-server/tests/not_leader.rs`, 3/3 passing) plus a direct check of
the published connector's error mapping: the `not_leader` code is not modelled by
0.3.x, so it falls back to the generic `AuraServerError` (acceptable for v0.4.1),
arrives with `retryable = False` (the wire frame omits the field, which the
connector defaults to false), and the connector retry policy is bounded
(`max_attempts = 3`), so a client never retries forever. No connector change was
required.

## Official client

The published Aura Connector (>= 0.3.0) drives the same server through its native
AuraDB backend, including auth and TLS. The documented Query IR shapes
(`docs/QUERY_ENGINE.md`) describe the wire-level contract. See
[AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md) and
[COMPATIBILITY.md](COMPATIBILITY.md).

Now that Aura Connector 0.3.0 is published, the `connector` job in
`.github/workflows/conformance.yml` is **active and required**: CI installs
`aura-connector>=0.3.0` from PyPI and runs
`tests/conformance/python/run_connector_conformance.py` against a freshly built
server. It is no longer a no-op gated on the package being unavailable. A planned
enhancement (see [ROADMAP](ROADMAP.md)) pins golden frame and IR fixtures from the
connector package so conformance is checked against the canonical client encoding.
