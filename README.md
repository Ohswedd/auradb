<div align="center">

# AuraDB

**A single-node, Rust-native multi-model database server for the Aura ecosystem.**

[![CI](https://github.com/Ohswedd/auradb/actions/workflows/ci.yml/badge.svg)](https://github.com/Ohswedd/auradb/actions/workflows/ci.yml)
[![Security](https://github.com/Ohswedd/auradb/actions/workflows/security.yml/badge.svg)](https://github.com/Ohswedd/auradb/actions/workflows/security.yml)
[![Docker](https://github.com/Ohswedd/auradb/actions/workflows/docker.yml/badge.svg)](https://github.com/Ohswedd/auradb/actions/workflows/docker.yml)
[![Release](https://img.shields.io/badge/release-v0.8.0-green.svg)](CHANGELOG.md)
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

AuraDB 0.5.0 introduces a **controlled, experimental multi-node server preview**:
real AuraDB server processes can form a cross-process cluster, elect a leader, and
replicate writes through Raft over a dedicated, frame-checked, authenticated peer
transport. The preview is **off by default** and gated behind two explicit
`[cluster]` opt-ins. **AuraDB v0.6.0 improves the controlled multi-node preview
and validates fail-stop recovery. It is _not_ production HA. Single-node mode
remains the recommended production mode.** v0.6.0 adds a leader kill / automatic
re-election preview, the first real peer snapshot install over the wire (a
bounded single-message transfer), larger follower catch-up coverage, sharper
fail-stop diagnostics, a published-image Docker Compose smoke, and peer
cert/token rotation and cluster backup/restore runbooks; it makes no
production-clustering or production-automatic-failover claims. **AuraDB v0.6.2
hardens repeated chaos and larger-state recovery for the controlled multi-node
preview**: repeated leader restart / re-election cycles, larger multi-model
data-set recovery, multi-model snapshot install, a peer reconnect storm,
deterministic network-interruption (partition/heal) simulations, and
recovery-focused diagnostics (`leader_changes`, reconnect-storm and
repeated-leader-change warnings). It is still _not_ production HA and single-node
mode remains the recommended production mode. See
[docs/V0_6_2_RELEASE_NOTES.md](docs/V0_6_2_RELEASE_NOTES.md). It builds on the v0.4.x Raft and replication
groundwork (a durable
consensus core, a replicated commit path, log compaction boundaries, and snapshot
restore hardening), and changes no on-disk or wire format.

**AuraDB v0.7.0 adds connector cluster ergonomics** (coordinated with Aura
Connector v0.4.0): the `not_leader` response carries an additive, structured
`not_leader` object — the leader's client address, the leader/current node ids,
term, role, and a usable `leader_hint` — so a connector can redirect to the
leader without parsing the message. The wire protocol (AWP 1) is unchanged and
older connectors ignore the new fields. It remains _not_ production HA;
single-node mode stays the recommended production mode. See
[docs/V0_7_RELEASE_NOTES.md](docs/V0_7_RELEASE_NOTES.md).

**AuraDB v0.7.1 is a connector-ergonomics polish release** (coordinated with Aura
Connector v0.4.1): clearer compatibility docs for Python cluster-preview users,
hardened connector cluster conformance guidance, and additional leader-hint and
safe-redirect examples. It adds **no** new database architecture and changes
neither the on-disk nor the wire format — the `not_leader` payload is byte-for-byte
the same as v0.7.0. It remains _not_ production HA; single-node mode stays the
recommended production mode. See
[docs/V0_7_1_RELEASE_NOTES.md](docs/V0_7_1_RELEASE_NOTES.md).

**AuraDB v0.8.0 is a production-readiness candidate for single-node and a
stronger cluster preview.** It is a hardening, validation, and operability
release: a structured `auradb check --json` consistency report with broad
corruption drills, a non-importing `auradb backup verify`, backup/restore and
upgrade drills over genuine fixtures, a new `[limits]` config section with five
enforced, configurable bounds, large-dataset / soak / performance tooling, a
security hardening review, cluster-preview recovery coverage, operator runbooks,
and release-artifact reproducibility. It adds **no** new database features and
changes neither the on-disk nor the wire format. It is **not** production HA;
single-node mode remains the recommended production mode. See
[docs/V0_8_RELEASE_NOTES.md](docs/V0_8_RELEASE_NOTES.md) and
[docs/PRODUCTION_READINESS.md](docs/PRODUCTION_READINESS.md).

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
not serializable isolation. It is honest about its boundaries. The multi-node
server preview is **experimental**: real cross-process leader election and Raft
replication work, but the preview is off by default, gated behind two opt-ins
(`enabled = true` and `experimental_multi_node = true`), uses static membership,
and a single-node cluster provides no fault tolerance. The following are not
implemented and not claimed: production multi-node clustering, automatic
failover, dynamic membership, linearizable reads, follower reads, distributed
transactions, sharding, multi-region; approximate (ANN/HNSW) vector indexes; BM25
full-text and hybrid fusion ranking; and serializable isolation.
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
Connector 0.4.x ships a native AuraDB-over-TCP backend that speaks AWP 1
(including auth and TLS) and adds cluster-preview ergonomics; see
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
  and CI. For single-node production guidance and operator runbooks, see
  [`docs/PRODUCTION_READINESS.md`](docs/PRODUCTION_READINESS.md) and
  [`docs/RUNBOOKS.md`](docs/RUNBOOKS.md).

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

See [`docs/CLUSTERING.md`](docs/CLUSTERING.md),
[`docs/RAFT.md`](docs/RAFT.md), and [`docs/REPLICATION.md`](docs/REPLICATION.md).

## New in 0.5.0: experimental multi-node preview

> **AuraDB v0.5.0 introduces a controlled, experimental multi-node server
> preview. Single-node mode remains the recommended production mode.**

Real AuraDB server processes can now form a cross-process cluster, elect a leader,
and replicate writes through Raft. The preview is **off by default** and requires
two explicit `[cluster]` opt-ins:

```toml
[cluster]
enabled = true
experimental_multi_node = true
```

- **Cross-process peer transport.** A dedicated, frame-checked (magic `APR1`,
  protocol v1, length-delimited, CRC32, 16 MiB cap) socket carries Raft messages;
  connections open with a `PeerHello` handshake that verifies the protocol
  version, cluster id, node id (against static membership), and a shared token.
- **Static membership.** Every node declares every other by `{ node_id, addr }`;
  there is no join/leave/dynamic membership.
- **Fail-closed guardrails.** A non-empty `peers` list requires
  `experimental_multi_node = true`; any non-loopback cluster address requires
  `allow_experimental_public_cluster = true`, which additionally requires peer TLS
  and a `peer_auth_token`.
- **Leader/follower behavior.** Writes go to the leader and commit on a majority
  (a minority cannot commit); followers reject writes with a structured
  `not_leader` error and reject reads. A restarted follower catches up from the
  leader.
- **Live tooling.** `auradb cluster leader|wait-leader|wait-ready` query a running
  node, and `auradb status --json` reports per-peer state (`preview_multi_node`,
  `quorum_available`, `peers`).

The validated path is the three-node loopback example. Try it from
[`examples/cluster`](examples/cluster) and read
[`docs/CLUSTERING.md`](docs/CLUSTERING.md):

```bash
auradb server --config examples/cluster/node1.toml
auradb server --config examples/cluster/node2.toml
auradb server --config examples/cluster/node3.toml
auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30
auradb cluster status      --addr 127.0.0.1:7171 --json
```

This is an experimental preview for local testing and early validation only; it
is not production multi-node clustering and has no automatic failover.

## What is intentionally not claimed yet

Production multi-node clustering, automatic failover, dynamic membership,
linearizable reads, follower reads, distributed transactions, sharding, and
multi-region (the 0.5.0 multi-node path is an experimental, opt-in preview);
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

Aura Connector talks to AuraDB over AWP. The published Aura Connector 0.4.x
ships a native AuraDB-over-TCP backend that speaks AWP 1, including auth and TLS.
Point it at the server address:

```bash
python -m pip install "aura-connector>=0.4,<0.5"
python tests/conformance/python/run_connector_smoke.py --addr 127.0.0.1:7171 \
  --auth-token "your-secret" --tls-ca .local/certs/ca.crt
```

For the multi-node preview, Aura Connector 0.4.x maps a `not_leader` response to a
dedicated `AuraNotLeaderError` and offers safe `connect_to_leader` / bounded
`with_leader_redirect` helpers; the cluster conformance runner is
`tests/conformance/python/run_connector_cluster.py`. Aura Connector 0.3.x stays
compatible (it routes the leader manually). Aura Connector 0.2.x uses a different
internal framing and is not wire compatible. See
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
auradb bench --json --output benches/baseline/v0.5.1.json
auradb status --addr 127.0.0.1:7171 --json
auradb cluster leader --addr 127.0.0.1:7171 --json
auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30
auradb cluster wait-ready --addr 127.0.0.1:7171 --timeout-secs 30
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
| `auradb cluster leader` | Report the leader a running server recognizes (`--addr`, `--json`) |
| `auradb cluster wait-leader` | Block until a running server reports a leader (`--addr`, `--timeout-secs`) |
| `auradb cluster wait-ready` | Block until a running server reports ready (`--addr`, `--timeout-secs`) |

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
docker run --rm -p 7171:7171 -v auradb-data:/data ghcr.io/ohswedd/auradb:0.5.1
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
auradb bench --json --output benches/baseline/v0.5.1.json
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
