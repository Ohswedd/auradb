# CLI

The `auradb` binary (crate `auradb-cli`) is the operator interface. Every command
performs a real operation against the engine or a running server.

## Commands

### `auradb version`
Prints the version.

### `auradb init --data-dir <dir> --config <file>`
Creates the data directory, initializes storage (manifest, catalog, first
segment), and writes a default config file.

### `auradb server [--config <file>] [--data-dir <dir>] [--bind <addr>] [--port <n>] [--allow-insecure-bind]`
Starts the server, loading config from a file and applying flag overrides. Runs
until Ctrl-C (graceful shutdown, which also writes fresh index snapshots).
`--allow-insecure-bind` permits a non-loopback bind while authentication is
disabled; without it, such a bind is rejected at startup.

### `auradb doctor --data-dir <dir> [--config <file>]`
Validates the config and data directory, opens the engine, and reports
collection/record counts, schema version, and index consistency. Also prints a
redacted security summary (bind, whether the bind is public, auth status, TLS
status); it never prints the token hash or any secret.

### `auradb status --addr <host:port> [--token <t>] [--tls-ca <ca>] [--tls-server-name <name>]`
Connects to a running server, pings it, and prints the health report. `--token`
authenticates when the server requires it; `--tls-ca` trusts a CA for a TLS
connection; `--tls-server-name` overrides the expected server name.

### `auradb check --data-dir <dir>`
Opens the engine and verifies on-disk index consistency, validating and
preserving persisted index snapshots.

### `auradb compact --data-dir <dir>`
Compacts the storage log, reporting segment counts and live records retained,
and writes fresh index snapshots as a checkpoint.

### `auradb dump --data-dir <dir> --out <file>`
Exports all schemas and records to JSONL (one schema/record per line).

### `auradb restore --data-dir <dir> --in <file>`
Recreates schemas and upserts records from a JSONL dump.

### `auradb bench --data-dir <dir> [--records <n>]`
Inserts `n` records and measures insert throughput, full-scan latency, and exact
vector search latency. Numbers are measured, never fabricated.

### `auradb auth hash-token [--token <t>]`
Generates an Argon2id token hash for the `[auth]` config block. Omit `--token` to
be prompted without echo. Prints a `$argon2id$...` string to paste into
`token_hash`. See [SECURITY.md](SECURITY.md).

### `auradb cert generate-dev [--out-dir <dir>]`
Generates development-only TLS certificates: `ca.crt`, `ca.key`, `server.crt`,
and `server.key`. The server certificate has SANs `localhost` and `127.0.0.1`,
signed by the generated development CA. Development use only.

### `auradb config validate [--config <file>]`
Validates a config file without starting the server. Fails on invalid values or
unsafe configuration (for example a public bind without auth, or auth/TLS enabled
without complete material).

### `auradb compatibility`
Prints the AuraDB version, the AWP protocol version, advertised server
capabilities, and the tested Aura Connector version. See
[COMPATIBILITY.md](COMPATIBILITY.md).

### `auradb index check [--data-dir <dir>]`
Reports how indexes loaded on open (how many from a snapshot and how many were
rebuilt) and verifies consistency.

### `auradb index rebuild [--data-dir <dir>]`
Rebuilds indexes from storage and persists fresh snapshots.

## Examples

```bash
auradb init --data-dir .local/auradb --config AuraDB.toml
auradb auth hash-token --token "your-secret"
auradb cert generate-dev --out-dir .local/certs
auradb config validate --config AuraDB.toml
auradb server --data-dir .local/auradb --port 7171 &
auradb status --addr 127.0.0.1:7171 --token "your-secret" --tls-ca .local/certs/ca.crt
auradb index check --data-dir .local/auradb
auradb dump --data-dir .local/auradb --out backup.jsonl
auradb restore --data-dir .local/restored --in backup.jsonl
auradb check --data-dir .local/restored
```

## Tests

`cmd_init`/`doctor`, `dump`→`restore` roundtrip, `check`, `compact`, `bench`,
`auth hash-token`, `cert generate-dev`, `config validate`, and `index
check`/`rebuild` are unit-tested; `server`/`status` are exercised by the smoke
test and conformance suite.
