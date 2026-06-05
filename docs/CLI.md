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

### `auradb doctor --data-dir <dir> [--config <file>] [--json]`
Validates the config and data directory, opens the engine, and reports
collection/record counts, schema version, and index consistency. Also prints a
redacted security summary (bind, whether the bind is public, auth status, TLS
status); it never prints the token hash or any secret. `--json` emits the report
as JSON (version, protocol version, data directory, storage/catalog/index status,
consistency result, and the redacted security summary).

### `auradb status --addr <host:port> [--token <t>] [--tls-ca <ca>] [--tls-server-name <name>] [--json]`
Connects to a running server, pings it, and prints the health report. `--token`
authenticates when the server requires it; `--tls-ca` trusts a CA for a TLS
connection; `--tls-server-name` overrides the expected server name. `--json`
emits the report (address, reachability, status, readiness, server version,
protocol version, collection count, and whether TLS was used).

### `auradb check --data-dir <dir>`
Opens the engine and verifies on-disk index consistency, validating and
preserving persisted index snapshots.

### `auradb gc [--data-dir <dir>] [--dry-run] [--json]`
Runs version garbage collection over the MVCC version chains. It reclaims versions
no active transaction can observe and drops fully-deleted records, always
retaining the latest version and at least `min_retained_versions`, and reports the
versions and records reclaimed plus the bytes reclaimed. `--dry-run` reports what
would be reclaimed without modifying any data; `--json` emits a machine-readable
report. See [STORAGE_ENGINE.md](STORAGE_ENGINE.md) and [OPERATIONS.md](OPERATIONS.md).

### `auradb stats analyze [--data-dir <dir>]`
Recomputes planner statistics (row counts, per-field cardinality, vector counts,
full-text document counts, average record size) by scanning the collections and
persists them to `planner_stats.json`. See [QUERY_ENGINE.md](QUERY_ENGINE.md).

### `auradb stats show [--data-dir <dir>] [--json]`
Prints the persisted planner statistics. `--json` emits the report as JSON.
Statistics are advisory: a missing file is reported, not an error.

### `auradb compact --data-dir <dir>`
Compacts the storage log, reporting segment counts and live records retained,
writes fresh index snapshots as a checkpoint, and refreshes planner statistics.

### `auradb dump --data-dir <dir> --output <file>`
Exports all schemas and records to JSONL (one schema/record per line).
Collections are written in dependency order so that a relationship target is
restored before the collection that references it. `--out` is an accepted alias
of `--output`.

### `auradb restore --data-dir <dir> --input <file>`
Recreates schemas and upserts records from a JSONL dump. `--in` is an accepted
alias of `--input`.

### `auradb bench --data-dir <dir> [--records <n>] [--json] [--output <file>]`
Runs the benchmark suite (storage append, point lookup, secondary-index lookup,
document-path lookup, full-text lookup, exact vector nearest, cursor paging,
frame encode/decode, and a dump/restore round trip). `--json` emits a full
report; `--output` writes the JSON report to a file (and implies `--json`).
Numbers are measured, never fabricated. See [BENCHMARKS.md](BENCHMARKS.md).

### `auradb bench compare --baseline <file> --current <file> [--fail-threshold-percent <p>]`
Compares two benchmark reports and prints the per-benchmark percent change,
marking regressions (slower throughput, or higher latency/wall time). By default
it only warns and exits 0; pass `--fail-threshold-percent` to exit non-zero when
any benchmark regresses by more than that percentage (for intentional CI gating).
Benchmarks are hardware- and load-sensitive — compare only reports produced on the
same quiescent machine.

The `auradb status` and `auradb doctor` commands additionally report an MVCC
section (active transactions, oldest snapshot age, retained versions, timeouts,
GC state) and, for `doctor`, operational warnings. See
[OBSERVABILITY.md](OBSERVABILITY.md) and [OPERATIONS.md](OPERATIONS.md).

### `auradb auth hash-token [--token <t>]`
Generates an Argon2id token hash for the `[auth]` config block. Omit `--token` to
be prompted without echo. Prints a `$argon2id$...` string to paste into
`token_hash`. See [SECURITY.md](SECURITY.md).

### `auradb auth rotate-token --config <file> [--token <t>] [--backup]`
Replaces the static token in a config file with a new Argon2id hash. The new
token is hashed, the config is rewritten atomically with unrelated fields
preserved, and the result is re-read and validated. With `--backup`, the previous
config is copied to `<config>.bak` first. Omit `--token` to be prompted without
echo. The plaintext token is never stored or printed. A running server keeps the
token it loaded at startup; restart it to enforce the new token. See
[SECURITY.md](SECURITY.md).

### `auradb cert generate-dev [--out-dir <dir>]`
Generates development-only TLS certificates: `ca.crt`, `ca.key`, `server.crt`,
and `server.key`. The server certificate has SANs `localhost` and `127.0.0.1`,
signed by the generated development CA. Development use only.

### `auradb config validate [--config <file>] [--no-file-checks]`
Validates a config file without starting the server. Fails on invalid values or
unsafe configuration (for example a public bind without auth, or auth/TLS enabled
without complete material). `--no-file-checks` validates structure without
requiring referenced TLS files to exist on disk, which is useful for validating a
deployment template whose certificates live on the target host; every other check
still applies.

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
auradb auth rotate-token --config AuraDB.toml --token "new-secret" --backup
auradb cert generate-dev --out-dir .local/certs
auradb config validate --config examples/auradb.secure.toml --no-file-checks
auradb server --data-dir .local/auradb --port 7171 &
auradb status --addr 127.0.0.1:7171 --token "your-secret" --tls-ca .local/certs/ca.crt --json
auradb index check --data-dir .local/auradb
auradb gc --data-dir .local/auradb
auradb stats analyze --data-dir .local/auradb
auradb stats show --data-dir .local/auradb --json
auradb dump --data-dir .local/auradb --output backup.jsonl
auradb restore --data-dir .local/restored --input backup.jsonl
auradb check --data-dir .local/restored
auradb bench --json --output benches/baseline/v0.3.0.json
```

## Tests

`cmd_init`/`doctor` (text and JSON), `dump`→`restore` roundtrip, `check`,
`compact`, `bench` (text and JSON), `auth hash-token`, `auth rotate-token`, `cert
generate-dev`, `config validate` (full and structural), `index
check`/`rebuild`, `gc`, and `stats analyze`/`show` are unit-tested. A dedicated backup/restore integration test
(`crates/auradb-cli/tests/backup_restore.rs`) exercises `dump`/`restore`/`check`
across every field and index kind. `server`/`status` are exercised by the smoke
test and conformance suite.
