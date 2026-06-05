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
