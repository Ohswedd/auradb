# CLI

The `auradb` binary (crate `auradb-cli`) is the operator interface. Every command
performs a real operation against the engine or a running server.

## Commands

### `auradb version`
Prints the version.

### `auradb init --data-dir <dir> --config <file>`
Creates the data directory, initializes storage (manifest, catalog, first
segment), and writes a default config file.

### `auradb server [--config <file>] [--data-dir <dir>] [--bind <addr>] [--port <n>]`
Starts the server, loading config from a file and applying flag overrides. Runs
until Ctrl-C (graceful shutdown).

### `auradb doctor --data-dir <dir> [--config <file>]`
Validates the config and data directory, opens the engine, and reports
collection/record counts, schema version, and index consistency.

### `auradb status --addr <host:port>`
Connects to a running server, pings it, and prints the health report.

### `auradb check --data-dir <dir>`
Opens the engine and verifies on-disk index consistency.

### `auradb compact --data-dir <dir>`
Compacts the storage log, reporting segment counts and live records retained.

### `auradb dump --data-dir <dir> --out <file>`
Exports all schemas and records to JSONL (one schema/record per line).

### `auradb restore --data-dir <dir> --in <file>`
Recreates schemas and upserts records from a JSONL dump.

### `auradb bench --data-dir <dir> [--records <n>]`
Inserts `n` records and measures insert throughput, full-scan latency, and exact
vector search latency. Numbers are measured, never fabricated.

## Examples

```bash
auradb init --data-dir .local/auradb --config AuraDB.toml
auradb server --data-dir .local/auradb --port 7171 &
auradb status --addr 127.0.0.1:7171
auradb dump --data-dir .local/auradb --out backup.jsonl
auradb restore --data-dir .local/restored --in backup.jsonl
auradb check --data-dir .local/restored
```

## Tests

`cmd_init`/`doctor`, `dump`→`restore` roundtrip, `check`, `compact`, and `bench`
are unit-tested; `server`/`status` are exercised by the smoke test and
conformance suite.
