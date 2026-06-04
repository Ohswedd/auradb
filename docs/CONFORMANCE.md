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

## CI

The `conformance.yml` workflow runs the Python harness against a live server in
three configurations: auth disabled, auth enabled (including a check that
unauthenticated requests are rejected), and TLS.

## Status

All Rust scenarios pass in CI via `cargo test`. The Python harness passes all
scenarios against a locally running server.

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
