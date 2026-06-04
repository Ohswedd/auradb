# AuraDB Python Conformance Harness

`run_conformance.py` is a self-contained Aura Wire Protocol client (Python
standard library only) that connects to a running AuraDB server and runs the
full first-release conformance scenario suite.

## Run it

Start a server, then run the harness against it:

```bash
# Terminal 1 - start AuraDB
cargo run --release -p auradb-cli -- server \
  --data-dir .local/auradb-dev --bind 127.0.0.1 --port 7171

# Terminal 2 - run the harness
python tests/conformance/python/run_conformance.py --addr 127.0.0.1:7171
```

The script prints a PASS/FAIL line per scenario and exits non-zero if any fail.

## Scenarios

connect, ping, health, schema create, insert, find, filter, document field,
relationship include, vector nearest, explain, count, exists, migration
estimate, update, upsert, delete, and transaction commit/rollback.

## Using the official Aura Connector

When the official client package is published, install it and adapt the client
in `run_conformance.py`:

```bash
python -m pip install aura-connector
```

The IR/JSON payload shapes the harness sends are the documented Query IR
(`docs/QUERY_ENGINE.md`); a follow-up task pins golden fixtures from the
published connector.
