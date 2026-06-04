# AuraDB

[![CI](https://github.com/Ohswedd/auradb/actions/workflows/ci.yml/badge.svg)](https://github.com/Ohswedd/auradb/actions/workflows/ci.yml)
[![Security](https://github.com/Ohswedd/auradb/actions/workflows/security.yml/badge.svg)](https://github.com/Ohswedd/auradb/actions/workflows/security.yml)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Release](https://img.shields.io/badge/release-v0.1.0-green.svg)](CHANGELOG.md)

AuraDB is a single-node, Rust-native database server for the Aura ecosystem. It
speaks the Aura Wire Protocol, persists data locally, and provides typed schema,
document fields, relationship includes, exact vector search, transactions,
cursors, observability, and CLI tooling.

This repository is the database engine side of the Aura ecosystem. It implements
a real, persistent, recoverable, single-node server, not a mock or an in-memory
demo.

## Scope and honesty

AuraDB 0.1.0 is an early single-node developer release. It is a complete
single-node server, and it is honest about its boundaries. The following are not
implemented and not claimed in this release: distributed clustering,
replication, sharding, failover, multi-region, and Raft; approximate (ANN/HNSW)
vector indexes; BM25 full-text and hybrid fusion ranking; serializable MVCC; and
enforced TLS or authentication. Unsupported operations return a structured
error. See [Limitations](#limitations) and the [roadmap](docs/ROADMAP.md). Do
not use this release for mission-critical deployments.

## Why AuraDB

Modern AI applications often split state across a relational database, a document
store, a graph database, and a vector database, then duplicate permissions and
struggle to keep them consistent. AuraDB explores the intersection: records that
are simultaneously rows, documents, graph nodes, and embedding holders, in one
transactional store. The forward direction is captured in the
[roadmap](docs/ROADMAP.md).

## How it relates to Aura Connector

Aura Connector is the client; AuraDB is the server. AuraDB implements the Aura
Wire Protocol (AWP) and an Aura-Connector-compatible Query IR. The conformance
suite ([`crates/auradb-conformance`](crates/auradb-conformance)) exercises every
first-release capability over the wire, and a Python harness lives in
[`tests/conformance/python`](tests/conformance/python).

## What works in 0.1.0

- **Persistent storage.** Append-only checksummed segment log, manifest, crash
  recovery, corruption detection, and compaction.
- **Transactions.** Atomic commit, rollback, optimistic conflict detection,
  read-your-writes, and crash recovery.
- **Schema catalog.** Typed fields, primary keys, unique and secondary indexes,
  document and vector fields, relationships, and validation.
- **Query engine.** Find, filter (`=, !=, <, <=, >, >=, in`, `contains`,
  `AND/OR/NOT`, document paths), order/limit/offset, projection, count, exists,
  insert/bulk/update/delete/upsert, relationship includes, exact vector
  nearest-neighbour search, and `EXPLAIN`.
- **Operations.** Migration impact estimation, server-side cursors,
  observability (metrics and tracing), a full CLI, Docker support, and CI.

## What is intentionally not claimed yet

Distributed clustering, replication, sharding, failover, multi-region, and Raft;
ANN/HNSW vector indexes; BM25 full-text and hybrid fusion ranking; serializable
MVCC; enforced TLS and authentication; RBAC, field-level encryption, audit
logging; time-travel queries; and change streams. These are tracked in the
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

## Connect with Aura Connector

Aura Connector talks to AuraDB over AWP. When the official `aura-connector`
Python package is installed, point it at the server address:

```bash
python -m pip install aura-connector
python tests/conformance/python/run_conformance.py --addr 127.0.0.1:7171
```

The Rust conformance client in `crates/auradb-conformance` stands in for the
client in automated tests and exercises the same scenarios over the wire.

## CLI examples

```bash
auradb version
auradb init --data-dir .local/auradb
auradb doctor --data-dir .local/auradb
auradb check --data-dir .local/auradb
auradb bench --data-dir .local/auradb
auradb status --addr 127.0.0.1:7171
```

| Command | Description |
|---|---|
| `auradb version` | Print the version |
| `auradb init` | Create a data directory and config file |
| `auradb server` | Start the server |
| `auradb doctor` | Validate config and data directory |
| `auradb status` | Ping a running server and report health |
| `auradb check` | Verify on-disk index consistency |
| `auradb compact` | Compact the storage log |
| `auradb dump` | Export schemas and records to JSONL |
| `auradb restore` | Restore from a JSONL dump |
| `auradb bench` | Run a local insert/read/vector benchmark |

See [`docs/CLI.md`](docs/CLI.md).

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
includes, exact vector nearest-neighbour search, `EXPLAIN`, and a migration
impact estimate. Unsupported operations return a structured capability error. See
[`docs/QUERY_ENGINE.md`](docs/QUERY_ENGINE.md).

## Storage and transaction model

Storage is an append-only, CRC32C-checksummed segment log with a manifest. On
open, the engine replays segments to rebuild the live record map and indexes; a
torn tail record is detected by checksum and truncated. Transactions buffer a
write set, then commit by acquiring the engine write lock, performing optimistic
write-write conflict detection against the versions read at transaction start,
and appending a commit batch atomically with the commit marker written last.
Rollback discards the write set. The isolation model is snapshot reads with
optimistic conflict detection on commit; serializable MVCC is not claimed. See
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

```bash
docker compose up --build
# server is now listening on localhost:7171

# Or build and run the image directly.
docker build -t auradb:local .
docker run --rm auradb:local auradb version
```

## Testing

```bash
cargo test --workspace --all-features
```

Tests span unit, integration (a real server over TCP), recovery (restart and
torn-tail), and conformance. See [`docs/TESTING.md`](docs/TESTING.md).

## Benchmarks

Criterion benchmarks measure real code with no fabricated numbers:

```bash
cargo bench --workspace
```

Targets cover frame encode and decode, storage writes and reads, indexed versus
full-scan queries, exact vector search, and cursor paging.

## Security model and current limits

Payload limits, frame validation, fail-closed storage, and
`#![forbid(unsafe_code)]` are in place across every crate. TLS and authentication
are configuration shapes only in this release and are not enforced; a TLS
configuration fails closed so plaintext is never served under it. Run AuraDB on a
trusted network or behind a TLS-terminating, authenticating proxy. See
[`SECURITY.md`](SECURITY.md) and [`docs/SECURITY.md`](docs/SECURITY.md).

## Roadmap

Planned directions, including ANN vector indexes, full-text and hybrid ranking,
enforced TLS and authentication, RBAC, persisted secondary indexes, document
path indexes, change streams, time travel, and distribution, are described in
[`docs/ROADMAP.md`](docs/ROADMAP.md).

## Contributing

Contributions are welcome. Please read [`CONTRIBUTING.md`](CONTRIBUTING.md) and
the [Code of Conduct](CODE_OF_CONDUCT.md) before opening a pull request.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
