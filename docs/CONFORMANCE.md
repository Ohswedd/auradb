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
   running server. It demonstrates cross-language wire compatibility.

## Scenarios

connect, ping, health, schema create, schema get/list, insert, find, filter,
document field, relationship include, vector nearest, explain, count, exists,
migration estimate, update, upsert, delete, transaction commit, transaction
rollback, and cursor streaming (forced via a small page size in the Rust test).

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

## Status

All Rust scenarios pass in CI via `cargo test`. The Python harness passes all
scenarios against a locally running server.

## Official client

When the official `aura-connector` Python package is published, the documented
Query IR shapes (`docs/QUERY_ENGINE.md`) let it run against AuraDB directly. A
planned enhancement (see [ROADMAP](ROADMAP.md)) pins golden frame and IR fixtures
from that package so conformance is checked against the canonical client encoding.
