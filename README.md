<div align="center">

# AuraDB

**A single-node, Rust-native multi-model database server for the Aura ecosystem.**

[![CI](https://github.com/Ohswedd/auradb/actions/workflows/ci.yml/badge.svg)](https://github.com/Ohswedd/auradb/actions/workflows/ci.yml)
[![Security](https://github.com/Ohswedd/auradb/actions/workflows/security.yml/badge.svg)](https://github.com/Ohswedd/auradb/actions/workflows/security.yml)
[![Docker](https://github.com/Ohswedd/auradb/actions/workflows/docker.yml/badge.svg)](https://github.com/Ohswedd/auradb/actions/workflows/docker.yml)
[![Release](https://img.shields.io/badge/release-v0.4.1-green.svg)](CHANGELOG.md)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

</div>

**Client library:** [Aura Connector](https://github.com/Ohswedd/aura-connector) (Python).

AuraDB is a single-node, Rust-native database server for the Aura ecosystem. It
speaks the Aura Wire Protocol, persists data locally, and provides typed schema,
document fields, relationship includes, exact vector search, transactions,
cursors, observability, and CLI tooling. As of 0.2.0 it also provides enforced
token authentication, server-terminated TLS, persisted index snapshots,
document-path indexes, and basic full-text search. As of 0.3.0 it adds MVCC
storage with single-node snapshot isolation and a cost-based query planner with
`EXPLAIN ANALYZE`.

This repository is the database engine side of the Aura ecosystem. It implements
a real, persistent, recoverable, single-node server, not a mock or an in-memory
demo.

## Scope and honesty

AuraDB 0.4.1 is a patch release that hardens the Raft and replication groundwork
introduced in 0.4.0 — log compaction boundaries, snapshot restore edge cases,
apply idempotency under restart, cluster-metadata corruption handling, peer
configuration validation, and operational diagnostics — before any cross-process
multi-node preview. **Multi-node server deployment remains experimental and
disabled by default; single-node mode remains the recommended production path.**

AuraDB 0.4.0 adds the replication and Raft foundation for future clustered
deployments, on top of the 0.3.x MVCC and query-planner foundations: each record
keeps a chain of committed versions, transactions read from a snapshot pinned at
`begin`, and read queries route through a cost-based planner with `EXPLAIN
ANALYZE`. **Single-node mode remains the recommended production path.** v0.4.0
introduces stable node and cluster identity, a durable Raft log, a deterministic
Raft state machine, a leader-only write path with a structured `not_leader`
error, an idempotent replicated apply path, a snapshot boundary, and cluster
CLI/metrics/status — all validated by tests. When cluster mode is disabled (the
default), every v0.3.1 behavior is preserved byte-for-byte. AuraDB implements
single-node snapshot isolation with optimistic write conflict detection. It is
not serializable isolation. It is honest about its boundaries. Multi-node
clustering is **experimental**: the Raft and replication core is exercised by
deterministic in-process tests, but cross-process multi-node server deployment is
not enabled in this release (configuring peers is rejected at startup), and a
single-node cluster provides no fault tolerance. The following are not
implemented and not claimed: production multi-node clustering, automatic
failover, linearizable reads, distributed transactions, sharding, multi-region;
approximate (ANN/HNSW) vector indexes; BM25 full-text and hybrid fusion ranking;
and serializable isolation.
Authentication and TLS are now implemented and enforced
when enabled, but RBAC, field-level encryption, encryption at rest, and audit
logging are not. Unsupported operations return a structured error. See
[Limitations](#security-model-and-current-limits) and the
[roadmap](docs/ROADMAP.md). Do not use this release for mission-critical
deployments.

## Why AuraDB

Modern AI applications often split state across a relational database, a document
store, a graph database, and a vector database, then duplicate permissions and
struggle to keep them consistent. AuraDB explores the intersection: records that
are simultaneously rows, documents, graph nodes, and embedding holders, in one
transactional store. The forward direction is captured in the
[roadmap](docs/ROADMAP.md).

## How it relates to Aura Connector

[Aura Connector](https://github.com/Ohswedd/aura-connector) is the client; AuraDB
is the server. AuraDB implements the Aura Wire Protocol (AWP) and an
Aura-Connector-compatible Query IR. The conformance
suite ([`crates/auradb-conformance`](crates/auradb-conformance)) exercises every
capability over the wire, and a Python harness lives in
[`tests/conformance/python`](tests/conformance/python). The published Aura
Connector 0.3.x ships a native AuraDB-over-TCP backend that speaks AWP 1
(including auth and TLS); see
[`docs/AURA_CONNECTOR_COMPATIBILITY.md`](docs/AURA_CONNECTOR_COMPATIBILITY.md)
and [`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md).

## What works in 0.3.0

- **Persistent storage.** Append-only checksummed segment log (storage format v2
  with commit-timestamped version chains), manifest, crash recovery, corruption
  detection, and compaction.
- **MVCC and snapshot isolation.** Each record keeps an ordered chain of committed
  versions (a delete is a tombstone version). Transactions pin a read timestamp at
  `begin` and read from that snapshot; non-transactional reads see the latest
  committed state. Commit uses optimistic, first-committer-wins write-conflict
  detection. Version garbage collection (`auradb gc` and optional background GC)
  reclaims versions no active transaction can observe. See
  [`docs/TRANSACTIONS.md`](docs/TRANSACTIONS.md) and
  [`docs/STORAGE_ENGINE.md`](docs/STORAGE_ENGINE.md).
- **Query planner and `EXPLAIN ANALYZE`.** Read queries route through a cost-based
  planner that uses persisted statistics (`planner_stats.json`) to choose the most
  selective applicable index or a full scan. `EXPLAIN ANALYZE` reports measured
  execution metrics. See [`docs/QUERY_ENGINE.md`](docs/QUERY_ENGINE.md).
- **Persisted indexes.** Indexes are snapshotted to disk at checkpoints and
  loaded on open when a content fingerprint and schema shape match; otherwise the
  engine safely rebuilds from storage. See [`docs/INDEXING.md`](docs/INDEXING.md).
- **Transactions.** Atomic commit, rollback, optimistic conflict detection,
  read-your-writes, and crash recovery.
- **Schema catalog.** Typed fields, primary keys, unique and secondary indexes,
  document and vector fields, document-path and full-text indexes, relationships,
  and validation.
- **Query engine.** Find, filter (`=, !=, <, <=, >, >=, in`, `contains`,
  `contains_text`, `AND/OR/NOT`, document paths), order/limit/offset, projection,
  count, exists, insert/bulk/update/delete/upsert, relationship includes, exact
  vector nearest-neighbour search, and `EXPLAIN`.
- **Document-path indexes.** Equality acceleration on nested document values
  addressed by a dotted path. See [`docs/DOCUMENTS.md`](docs/DOCUMENTS.md).
- **Full-text search.** A tokenized inverted index with boolean-AND
  `contains_text` matching and term-frequency ranking (not BM25). See
  [`docs/FULL_TEXT.md`](docs/FULL_TEXT.md).
- **Security.** Enforced static-token authentication (Argon2id-hashed) and
  server-terminated TLS with optional mutual TLS (rustls), both fail-closed. In
  place token rotation with `auradb auth rotate-token`. See
  [`docs/SECURITY.md`](docs/SECURITY.md).
- **Deployment.** A secure Docker Compose example (auth and TLS enabled, non-root,
  no committed secret) and production configuration templates. See
  [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md).
- **Operations.** Migration impact estimation, server-side cursors,
  observability (metrics and tracing, plus `--json` health output for `status`
  and `doctor`), backup and restore, verified upgrade from a v0.1.0 data
  directory, a full CLI, a published Docker image, prebuilt binary release
  artifacts, a benchmark baseline ([`docs/BENCHMARKS.md`](docs/BENCHMARKS.md)),
  and CI.

## New in 0.4.0: replication and Raft groundwork

AuraDB 0.4.0 lays the foundation for future clustered deployments. **Single-node
mode remains the recommended production path.**

- **Stable identity.** `auradb init` creates a persistent node id and cluster id
  under `<data_dir>/cluster/`.
- **Durable Raft log + state machine.** A checksummed, crash-safe Raft log and a
  deterministic follower/candidate/leader state machine (elections, log
  replication, log repair, commit advancement), validated by deterministic
  in-process tests — including multi-node consensus.
- **Single-node cluster mode.** Opt in with `[cluster] enabled = true`. Every
  write is ordered through a durable local Raft log and replayed on restart; the
  MVCC commit timestamp is the Raft log index. A single-node cluster has no fault
  tolerance — it is for exercising the replication path, not for high availability.
- **Leader-only writes.** Followers reject writes with a structured `not_leader`
  error and a leader hint.
- **Snapshot boundary, metrics, and CLI.** A versioned snapshot manifest for
  future state transfer; Raft/replication metrics; and
  `auradb cluster init|status|peers|doctor|bootstrap`.

Multi-node clustering is **experimental and not enabled for server deployment**:
configuring `peers` is rejected at startup. See [`docs/CLUSTERING.md`](docs/CLUSTERING.md),
[`docs/RAFT.md`](docs/RAFT.md), and [`docs/REPLICATION.md`](docs/REPLICATION.md).

## What is intentionally not claimed yet

Production multi-node clustering, automatic failover, linearizable reads,
distributed transactions, sharding, and multi-region (the Raft/replication
foundation in 0.4.0 is single-node and experimental for multi-node);
ANN/HNSW vector indexes; BM25 full-text and hybrid fusion ranking; serializable
isolation; RBAC, field-level encryption, encryption at rest, and audit logging;
time-travel queries; and change streams. These are tracked in the
[roadmap](docs/ROADMAP.md).

## Quick start

```bash
# Build everything.
cargo build --release

# Initialize a data directory and config.
./target/release/auradb init --data-dir .local/auradb --config AuraDB.toml

# Start the server.
./target/release/auradb server --data-dir .local/auradb --bind 127.0.0.1 --port 7171

# In another shell: check it is healthy.
./target/release/auradb status --addr 127.0.0.1:7171
```

## Install and build from source

AuraDB builds with a stable Rust toolchain (1.85 or newer).

```bash
git clone https://github.com/Ohswedd/auradb.git
cd auradb
cargo build --release
# The server and CLI binary is target/release/auradb
```

## Run the server

```bash
./target/release/auradb server --data-dir .local/auradb --bind 127.0.0.1 --port 7171
```

The server listens for Aura Wire Protocol frames over TCP. Configuration can come
from `AuraDB.toml`, the data directory, and CLI flags.

Binding loopback (`127.0.0.1`) is local developer mode and may leave auth
disabled. Binding a non-loopback address (for example `0.0.0.0`) with auth
disabled is rejected at startup unless you set `allow_insecure_bind = true` in
config or pass `--allow-insecure-bind`.

## Authentication and TLS quickstart

Enforced static-token authentication and server-terminated TLS are both opt-in
and fail closed. Full details are in [`docs/SECURITY.md`](docs/SECURITY.md).

```bash
# Generate an Argon2id token hash to paste into the [auth] config block.
auradb auth hash-token --token "your-secret"
# Output: $argon2id$v=19$m=19456,t=2,p=1$...$...
```

```toml
[auth]
enabled = true
mode = "static-token"
token_hash = "$argon2id$v=19$m=19456,t=2,p=1$...$..."
token_hash_algorithm = "argon2id"
```

```bash
# Generate development-only certificates (CA, server cert/key) for local TLS.
auradb cert generate-dev --out-dir .local/certs
```

```toml
[tls]
enabled = true
cert_path = ".local/certs/server.crt"
key_path  = ".local/certs/server.key"
```

```bash
# Connect over TLS, trusting the dev CA, and present the token.
auradb status --addr 127.0.0.1:7171 --tls-ca .local/certs/ca.crt --token "your-secret"
```

Tokens are never stored or compared in plaintext, secrets are never logged or
echoed in error frames, and `auradb doctor` prints a redacted security summary.

## Connect with Aura Connector

Aura Connector talks to AuraDB over AWP. The published Aura Connector 0.3.x
ships a native AuraDB-over-TCP backend that speaks AWP 1, including auth and TLS.
Point it at the server address:

```bash
python -m pip install "aura-connector>=0.3,<0.4"
python tests/conformance/python/run_connector_smoke.py --addr 127.0.0.1:7171 \
  --auth-token "your-secret" --tls-ca .local/certs/ca.crt
```

Aura Connector 0.2.x uses a different internal framing and is not wire
compatible; use 0.3.x. See
[`docs/AURA_CONNECTOR_COMPATIBILITY.md`](docs/AURA_CONNECTOR_COMPATIBILITY.md).

The Rust conformance client in `crates/auradb-conformance` stands in for the
client in automated tests and exercises the same scenarios over the wire.

## CLI examples

```bash
auradb version
auradb init --data-dir .local/auradb
auradb doctor --data-dir .local/auradb --json
auradb check --data-dir .local/auradb
auradb gc --data-dir .local/auradb
auradb stats analyze --data-dir .local/auradb
auradb stats show --data-dir .local/auradb --json
auradb bench --json --output benches/baseline/v0.4.1.json
auradb status --addr 127.0.0.1:7171 --json
auradb auth hash-token --token "your-secret"
auradb auth rotate-token --config AuraDB.toml --token "new-secret" --backup
auradb cert generate-dev --out-dir .local/certs
auradb config validate --config examples/auradb.secure.toml --no-file-checks
auradb dump --data-dir .local/auradb --output backup.jsonl
auradb restore --data-dir .local/restored --input backup.jsonl
auradb index check --data-dir .local/auradb
```

| Command | Description |
|---|---|
| `auradb version` | Print the version |
| `auradb init` | Create a data directory and config file |
| `auradb server` | Start the server (`--allow-insecure-bind` to permit a public bind without auth) |
| `auradb doctor` | Validate config and data directory; print a redacted security summary (`--json`) |
| `auradb status` | Ping a running server and report health (`--token`, `--tls-ca`, `--tls-server-name`, `--json`) |
| `auradb check` | Verify on-disk index consistency |
| `auradb gc` | Reclaim record versions no active transaction can observe |
| `auradb stats analyze` | Recompute and persist planner statistics |
| `auradb stats show` | Print persisted planner statistics (`--json`) |
| `auradb compact` | Compact the storage log and write fresh index snapshots |
| `auradb dump` | Export schemas and records to JSONL (`--output`) |
| `auradb restore` | Restore from a JSONL dump (`--input`) |
| `auradb bench` | Run the local benchmark suite (`--json`, `--output`) |
| `auradb auth hash-token` | Generate an Argon2id token hash for the config |
| `auradb auth rotate-token` | Rotate the static token in a config file (`--backup`) |
| `auradb cert generate-dev` | Generate development-only TLS certificates |
| `auradb config validate` | Validate a config file (`--no-file-checks` for templates) |
| `auradb compatibility` | Print version, AWP version, capabilities, and tested connector version |
| `auradb index check` | Report how indexes loaded and verify consistency |
| `auradb index rebuild` | Rebuild indexes from storage and persist fresh snapshots |

See [`docs/CLI.md`](docs/CLI.md) and [`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md).

## Data model overview

A record belongs to a typed collection defined by a schema. A record can carry
scalar fields, nested document fields, relationship fields, and fixed-dimension
vector fields at the same time. Records are addressed by a stable logical id;
physical storage offsets are never exposed as durable identity. See
[`docs/DOCUMENTS.md`](docs/DOCUMENTS.md) and
[`docs/RELATIONSHIPS.md`](docs/RELATIONSHIPS.md).

## Query capabilities

The Query IR supports point reads, filters (comparisons, `in`, `contains`,
`AND`/`OR`/`NOT`, document path access), ordering, limit and offset, projection,
count, exists, insert, bulk insert, update, delete, upsert, relationship
includes, exact vector nearest-neighbour search, `EXPLAIN` (and `EXPLAIN
ANALYZE`), and a migration impact estimate. Reads route through a cost-based
planner. Unsupported operations return a structured capability error. See
[`docs/QUERY_ENGINE.md`](docs/QUERY_ENGINE.md).

## Storage and transaction model

Storage is an append-only, CRC32C-checksummed segment log with a manifest, using
MVCC version chains (storage format v2): each record id maps to an ordered chain
of committed versions stamped with a commit timestamp, and a delete is a
tombstone version. On open, the engine replays segments to rebuild the version
chains and indexes; a torn tail record is detected by checksum and truncated.
Transactions pin a read timestamp at `begin` and read from that snapshot
(committed state as of the snapshot, overlaid with the transaction's own staged
writes and deletes), uniformly across find, filter, count, exists, explain,
vector, document-path, full-text, relationship include, and cursor paging.
Non-transactional reads see the latest committed state. Commit acquires the
engine write lock, performs optimistic first-committer-wins write-conflict
detection, and appends a commit batch atomically. Rollback discards the staged
set. AuraDB v0.3.0 implements single-node snapshot isolation with optimistic
write conflict detection. It is not serializable isolation. See
[`docs/STORAGE_ENGINE.md`](docs/STORAGE_ENGINE.md) and
[`docs/TRANSACTIONS.md`](docs/TRANSACTIONS.md).

## Vector, document, and relationship support

- **Vectors.** Fixed-dimension vectors are validated and stored inline. Nearest
  search is exact, ranked by cosine, euclidean, or dot product, behind a
  `VectorIndex` trait. ANN is not claimed. See [`docs/VECTORS.md`](docs/VECTORS.md).
- **Documents.** JSON-like nested objects and arrays are stored, validated where
  declared, and filterable by path. See [`docs/DOCUMENTS.md`](docs/DOCUMENTS.md).
- **Relationships.** Forward links with hydration through query includes and
  referential consistency checks. See
  [`docs/RELATIONSHIPS.md`](docs/RELATIONSHIPS.md).

## Observability

A metrics registry exports counters, gauges, and latency histograms as JSON and
Prometheus text, covering request and query latency, storage and WAL latency,
bytes read and written, active connections and transactions, cursor counts, and
error counts. Structured tracing is built in, and health and readiness surfaces
are exposed. No external collector is required to run the server. See
[`docs/OBSERVABILITY.md`](docs/OBSERVABILITY.md).

## Docker usage

A published image is available on the GitHub Container Registry:

```bash
docker run --rm -p 7171:7171 -v auradb-data:/data ghcr.io/ohswedd/auradb:0.4.1
```

The image runs as a non-root user, exposes `7171`, stores data in the `/data`
volume, and ships a `HEALTHCHECK` that calls `auradb status`. This base image is
for development; it binds all interfaces with `--allow-insecure-bind`.

For a deployment, use `docker-compose.secure.yml`, which enables authentication
and TLS, runs as a non-root user with a read-only root filesystem, mounts a
config and a certificate directory, and injects the token hash from the
environment so no secret is committed. See [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md).

```bash
# Development image (build locally or pull from GHCR).
docker compose up --build
docker build -t auradb:local .
docker run --rm auradb:local auradb version

# Validate the secure deployment compose file.
docker compose -f docker-compose.secure.yml config
```

## Upgrading

The on-disk storage format moves to v2 (commit-timestamped version chains) in
0.3.0. A v0.1.0, v0.2.0, or v0.2.1 data directory (storage format v1) is migrated
to v2 transparently the first time v0.3.0 opens it: existing records become the
first committed version on each chain, and planner statistics are initialized. An
unknown future format is still rejected rather than opened. This is covered by
tests against committed v0.2.0 and v0.2.1 fixtures. Take a backup with `auradb
dump` first. See [`docs/UPGRADING.md`](docs/UPGRADING.md).

## Testing

```bash
cargo test --workspace --all-features
```

Tests span unit, integration (a real server over TCP), backup/restore, upgrade
from v0.2.0 and v0.2.1 data directories (the v1-to-v2 MVCC migration), snapshot
isolation and version GC, query planner and `EXPLAIN ANALYZE`, deterministic
chaos restart under write load, deterministic seeded recovery and corruption
tests (restart, torn-tail truncation, byte-flip detection, catalog and index
repair), and conformance. See [`docs/TESTING.md`](docs/TESTING.md).

## Benchmarks

Benchmarks measure real code with no fabricated numbers. Criterion
microbenchmarks run with `cargo bench --workspace` and cover frame encode and
decode, storage writes and reads, indexed versus full-scan queries, exact vector
search, cursor paging, MVCC version reads and GC, planner access-path selection,
and `EXPLAIN ANALYZE`.

The CLI also runs a baseline suite and writes a JSON snapshot:

```bash
auradb bench --json --output benches/baseline/v0.4.1.json
```

Benchmarks are hardware-dependent and exist to catch regressions on the same
machine, not as universal claims. See [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md)
and the committed baseline under `benches/baseline/`.

## Security model and current limits

Payload limits, frame validation, fail-closed storage, and
`#![forbid(unsafe_code)]` are in place across every crate. Static-token
authentication (Argon2id-hashed, constant-time verification) and
server-terminated TLS with optional mutual TLS (rustls) are implemented and
enforced when enabled; both fail closed, so plaintext is never served under a TLS
configuration and a public bind without auth is rejected at startup. Tokens and
other secrets are never logged or echoed. Not implemented: RBAC, field-level
encryption, encryption at rest, and audit logging. AuraDB is single node. See
[`SECURITY.md`](SECURITY.md) and [`docs/SECURITY.md`](docs/SECURITY.md).

## Roadmap

Planned directions, including ANN vector indexes, BM25 full-text and hybrid
ranking, serializable isolation, RBAC, field-level encryption, audit logging,
change streams, time travel, and distribution (replication, clustering, sharding,
Raft), are described in [`docs/ROADMAP.md`](docs/ROADMAP.md).

## Contributing

Contributions are welcome. Please read [`CONTRIBUTING.md`](CONTRIBUTING.md) and
the [Code of Conduct](CODE_OF_CONDUCT.md) before opening a pull request.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
