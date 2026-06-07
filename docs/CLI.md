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

### `auradb check --data-dir <dir> [--json]`
Opens the engine and verifies on-disk index consistency, validating and
preserving persisted index snapshots. The plain form prints `index consistency
OK; N records verified`. **(v0.8.0)** `--json` emits a structured consistency
report with top-level fields `ok`, `auradb_version`, `data_dir`, `storage`,
`catalog`, `indexes`, `planner_stats`, `raft`, `snapshots`, `warnings`, and
`errors`, and **exits non-zero if any check fails** (so it can be scheduled and
alerted on). The report covers segment checksums, the manifest, the catalog, the
index manifest (a recoverable mismatch is rebuilt and reported as a warning),
planner statistics (advisory — a problem is a warning, not a failure), the Raft
log, and snapshot boundaries; an unknown future storage format is rejected. The
report never prints secrets. See [STORAGE_ENGINE.md](STORAGE_ENGINE.md) and
[OBSERVABILITY.md](OBSERVABILITY.md).

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
alias of `--input`. The restore enforces a 64 MiB per-line bound on its input.

### `auradb backup verify --input <file> --json` (v0.8.0)
Validates a JSONL dump **without importing it**: it checks that every line parses,
that a per-line size bound holds, and that records reference declared schemas. The
`--json` report carries `ok`, `input`, `schemas`, `records`, a `collections` map
(per-collection counts), `warnings`, and `errors`, and the command **exits
non-zero on an invalid backup**. Run it after `auradb dump` and before relying on
a backup. See [OPERATIONS.md](OPERATIONS.md) and [UPGRADING.md](UPGRADING.md).

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

### `auradb cert generate-dev [--out-dir <dir>] [--server-name <name>] [--san <name>]...`
Generates development-only TLS certificates: a `ca.crt` / `ca.key` development CA
and a server certificate/key signed by it. With no arguments it writes
`server.crt` / `server.key` with SANs `localhost` and `127.0.0.1` (the original
behavior). With `--server-name nodeN` it sets the certificate Common Name and
writes `nodeN.crt` / `nodeN.key`, and `--san` (repeatable) sets the Subject
Alternative Names (defaulting to the server name plus `localhost` and
`127.0.0.1`). An existing CA in the output directory is reused, so several
per-node certificates can share one trust root — see
`examples/cluster/generate-dev-certs.sh`. Development use only.

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

### `auradb cluster ...` (v0.4.0)
Cluster (Raft) administration. These commands operate offline on a data
directory's cluster metadata and the parsed `[cluster]` configuration; they do not
stand up a running node.

- `auradb cluster init [--data-dir <dir>] [--config <file>]` — create stable node
  and cluster identity if not already present. (`auradb init` also creates node
  identity.)
- `auradb cluster status [--data-dir <dir>] [--config <file>] [--json]` — show
  local cluster metadata for a data directory. **(v0.5.1)** With `--addr
  <client-addr>` (plus optional `--token` / `--tls-ca` / `--server-name`) it
  instead queries a **running** server for live diagnostics: role, leader (and its
  client address), quorum availability, commit/applied/last-log indices,
  replication lag, and per-peer reachability (`connected`, `connect_attempts`,
  `match_index` / `next_index`). **(v0.6.1)** Each peer entry also carries
  snapshot/lag fields — `lag_entries`, `needs_snapshot`, `snapshot_in_progress`,
  and `catch_up_state` (`normal` / `probing` / `snapshot_needed` /
  `snapshot_installing` / `caught_up` / `unknown`) — alongside cluster-level
  snapshot diagnostics (last installed boundary, last install time, last error,
  bytes sent, bytes installed, in-progress gauge, needed-total). **(v0.6.2)** The
  report also includes `leader_changes`, the cumulative number of leadership
  changes this node has observed since it started (a leadership-instability
  signal).
- `auradb cluster peers [--data-dir <dir>] [--config <file>] [--json]` — list
  configured cluster peers.
- `auradb cluster doctor [--data-dir <dir>] [--config <file>] [--json]` — validate
  the cluster configuration and on-disk identity offline. **(v0.6.1)** With
  `--addr <client-addr>` (plus optional `--token` / `--tls-ca` / `--server-name`,
  and `--json`) it instead becomes a **live** check: it fetches live health from a
  running server and emits warnings for a follower that needs a snapshot, a lagging
  follower, and quorum at the minimum / quorum lost. **(v0.6.2)** It additionally
  warns on a **peer reconnect storm** (a peer still disconnected after many
  outbound connection attempts) and on **repeated leader changes** (the
  `leader_changes` count crossing an instability threshold). The offline
  `--data-dir` form is unchanged.
- `auradb cluster bootstrap [--data-dir <dir>] [--config <file>]` — bootstrap a
  brand-new single-node cluster identity.
- `auradb cluster compact-log [--data-dir <dir>] [--config <file>] [--dry-run]
  [--json]` (v0.4.1) — compact the durable Raft log up to the safely-applied
  prefix. `--dry-run` reports what would be discarded without modifying anything.
  Compaction never runs ahead of the committed/applied prefix. Requires an
  initialized single-node cluster.

### `auradb cluster backup-plan` / `auradb cluster restore-plan` (v0.6.1)

Two **dry-run planners**: both inspect and report only and **never write data**.

- `auradb cluster backup-plan --data-dir <dir> [--config <file>] [--json]` —
  inspects the data directory and config and reports the source mode
  (`leader-logical-backup` for a cluster node, else
  `local-data-dir-logical-backup`); what a logical backup would **include** (latest
  committed state, schema, collection and record counts; indexes rebuilt on
  restore); what it **excludes** (the Raft log and compaction state, cluster
  membership/peer metadata, uncommitted entries, historical MVCC versions); the
  restore target (single-node restore into a fresh data dir, optionally
  bootstrapping a fresh single-node preview cluster); referenced secrets (auth
  token, peer auth token, TLS material) shown **redacted** and noted as **not**
  included in the backup; and warnings (cannot restore directly into a live
  multi-node cluster; run from a stable leader with writes quiesced; verify after
  restore).
- `auradb cluster restore-plan --input <backup.jsonl> [--json]` — inspects a JSONL
  logical dump and reports its schema/record counts, collections, restore target,
  exclusions, and the same "no live multi-node restore" warning.

These planners describe the existing `auradb dump` / `auradb restore` flow; they
do not perform it. See [OPERATIONS.md](OPERATIONS.md) and [SECURITY.md](SECURITY.md).

### `auradb cluster leader|wait-leader|wait-ready` (v0.5.0)

These are **live** commands: unlike the offline `--data-dir` subcommands above,
they query a **running** server over its client address. Each accepts `--json`,
`--token`, `--tls-ca`, and `--server-name`.

- `auradb cluster leader --addr <client-addr>` — report the leader the running
  server currently recognizes (or that none is known yet).
- `auradb cluster wait-leader --addr <client-addr> --timeout-secs N` — block until
  the server reports a recognized leader, or the timeout elapses.
- `auradb cluster wait-ready --addr <client-addr> --timeout-secs N` — block until
  the server reports ready, or the timeout elapses.

`join`, `leave`, and `step-down` are **not provided**, because membership changes
are not implemented in this release. `auradb status --json` and `auradb doctor`
also include the cluster fields (including the v0.5.0 per-peer state). See
[CLUSTERING.md](CLUSTERING.md) and
[CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md).

### `auradb snapshot ...` (v0.4.1)
Capture, inspect, and restore a portable snapshot of a data directory. A snapshot
is a self-contained logical dump (schemas + current live records) with a versioned
manifest recording the cluster/node id, storage-format version, collection/record
counts, a digest, and a creation timestamp.

- `auradb snapshot create --data-dir <dir> --output <file>` — write a snapshot
  file. If the directory carries cluster identity, the snapshot records it.
- `auradb snapshot inspect --input <file>` — print the manifest and verify its
  payload digest (`integrity: ok`) without restoring.
- `auradb snapshot restore --input <file> --data-dir <dir> [--force]` — restore a
  snapshot into a data directory. The restore is **atomic** (built in a staging
  directory, validated, then swapped into place) and refuses to overwrite a
  non-empty directory unless `--force` is passed. Future formats, cluster-id
  mismatches, corrupt manifests, and digest mismatches are rejected before any
  existing data is touched.

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
