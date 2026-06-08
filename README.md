<div align="center">

# AuraDB

**Production-supported single-node database for typed records, documents, exact vectors, BM25-ranked text, and hybrid search.**

[![CI](https://github.com/Ohswedd/auradb/actions/workflows/ci.yml/badge.svg)](https://github.com/Ohswedd/auradb/actions/workflows/ci.yml)
[![Security](https://github.com/Ohswedd/auradb/actions/workflows/security.yml/badge.svg)](https://github.com/Ohswedd/auradb/actions/workflows/security.yml)
[![Docker](https://github.com/Ohswedd/auradb/actions/workflows/docker.yml/badge.svg)](https://github.com/Ohswedd/auradb/actions/workflows/docker.yml)
[![Release](https://img.shields.io/badge/release-v1.1.0-green.svg)](CHANGELOG.md)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

</div>

AuraDB is a Rust-native, single-node database server. One transactional store holds
records that are simultaneously typed rows, nested documents, graph nodes, and embedding
holders — so an AI application can keep relational state, document fields, relationships,
exact vectors, and ranked text search in one place instead of stitching together four
systems. It speaks the Aura Wire Protocol over TCP, persists and recovers data locally,
and ships with auth, TLS, backups, observability, and operator runbooks.

The matching client is [**Aura Connector**](https://github.com/Ohswedd/aura-connector), a
typed async Python connector. AuraDB v1.1.0 pairs with Aura Connector v0.5.x.

```bash
docker run --rm -p 7171:7171 -v auradb-data:/data ghcr.io/ohswedd/auradb:1.1.0
```

## Support status

| Mode / capability | Status | Production use |
| ----------------- | ------ | -------------- |
| Single-node with auth + TLS + backups + monitoring | Stable | **Yes (recommended)** |
| Backup / restore, upgrade from v0.x | Stable | **Yes** |
| Exact vector search, tokenized full-text | Stable | **Yes** |
| BM25 ranked full-text, hybrid text+vector search | Stable | **Yes** |
| Aura Connector 0.5.x, AWP 1, storage format v2 | Stable / frozen for v1 | **Yes** |
| Static multi-node cluster (Raft) | HA candidate preview | **No** (not production HA) |
| Approximate (ANN/HNSW) vector search | Not implemented | — |

**Single-node mode is the recommended production mode.** Multi-node static clustering is an
HA *candidate preview* with strong release-candidate evidence — it is **not** production HA,
has no production automatic failover, and is off by default. The authoritative boundaries
are in [`docs/SUPPORT_POLICY.md`](docs/SUPPORT_POLICY.md),
[`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md), and
[`docs/HA_RELEASE_CANDIDATE.md`](docs/HA_RELEASE_CANDIDATE.md).

## Install and run

```bash
# Docker (development image; binds all interfaces with --allow-insecure-bind).
docker run --rm -p 7171:7171 -v auradb-data:/data ghcr.io/ohswedd/auradb:1.1.0

# From source (stable Rust 1.85+). The server and CLI is one binary: target/release/auradb.
git clone https://github.com/Ohswedd/auradb.git && cd auradb
cargo build --release
./target/release/auradb init   --data-dir .local/auradb --config AuraDB.toml
./target/release/auradb server --data-dir .local/auradb --bind 127.0.0.1 --port 7171
./target/release/auradb status --addr 127.0.0.1:7171     # in another shell
```

Binding loopback (`127.0.0.1`) is local developer mode and may leave auth disabled. Binding
a non-loopback address with auth disabled is rejected at startup unless you explicitly opt
in. For a real deployment use the secure Compose file (auth + TLS, non-root, read-only root
filesystem, no committed secret): see [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md).

```bash
docker compose -f docker-compose.secure.yml config   # validate the secure stack
```

## Connect

Aura Connector talks to AuraDB over AWP 1, including auth and TLS. AuraDB v1.1.0 is paired
with Aura Connector v0.5.x (search and ranking); v0.4.x connects for non-search operations.

```bash
python -m pip install "aura-connector>=0.5,<0.6"
```

```python
from aura import connect
from aura.config import TokenAuth, TLSConfig

async with connect(
    "auradbs://db.example.com:7171/app",
    models=[Doc],
    auth=TokenAuth("your-secret"),
    tls=TLSConfig(enabled=True, ca_cert_path="/etc/aura/ca.pem"),
) as client:
    await client.insert(Doc(id=1, title="Refunds", body="...", embedding=[0.1, 0.2, 0.3]))
```

The Rust conformance client in [`crates/auradb-conformance`](crates/auradb-conformance) and a
Python harness in [`tests/conformance/python`](tests/conformance/python) exercise every
capability over the wire. Compatibility is documented in
[`docs/AURA_CONNECTOR_COMPATIBILITY.md`](docs/AURA_CONNECTOR_COMPATIBILITY.md) and
[`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md).

## Data model

A record belongs to a typed collection defined by a schema and can carry, at the same time:

- **Typed scalar fields** — `uuid`, `string`, `int`, `float`, `bool`, `timestamp`, `bytes`,
  with primary keys, unique and secondary indexes, and validation.
- **Document fields** — JSON-like nested objects/arrays, filterable and orderable by dotted
  path, with optional document-path equality indexes
  ([`docs/DOCUMENTS.md`](docs/DOCUMENTS.md)).
- **Relationships** — forward links hydrated through query `include`, with referential
  consistency on delete ([`docs/RELATIONSHIPS.md`](docs/RELATIONSHIPS.md)).
- **Vectors** — fixed-dimension embeddings stored inline, validated by dimension
  ([`docs/VECTORS.md`](docs/VECTORS.md)).

Records are addressed by a stable logical id; physical storage offsets are never exposed as
durable identity.

## Query and search

Reads route through a cost-based planner that uses persisted statistics to pick the most
selective index or a full scan, with `EXPLAIN` and `EXPLAIN ANALYZE`. The Query IR supports
point reads, filters (`=, !=, <, <=, >, >=, in`, `contains`, `contains_text`, `AND/OR/NOT`,
document paths), ordering, limit/offset, projection, count, exists,
insert/bulk/update/delete/upsert, relationship includes, and a migration impact estimate.

Search and ranking ([`docs/SEARCH_AND_RANKING.md`](docs/SEARCH_AND_RANKING.md)) adds:

- **BM25 ranked full-text** (`text_search`) — Okapi BM25 over a full-text indexed field, with
  tunable `k1`/`b`.
- **Exact vector search** (`vector`) — exact nearest-neighbour by `cosine`, `euclidean`, or
  `dot_product`. This is the correctness baseline; ANN/HNSW is not implemented.
- **Hybrid text + vector** (`hybrid`) — BM25 and vector signals fused by weighted sum or
  reciprocal-rank fusion.

```bash
# Inspect a ranked query's plan (and measured metrics with --analyze).
auradb search explain --input examples/search_bm25.json
auradb search explain --input examples/search_hybrid.json --analyze
```

The legacy `contains_text` boolean predicate and term-frequency ranking are unchanged
([`docs/FULL_TEXT.md`](docs/FULL_TEXT.md)). Unsupported operations return a structured
capability error.

## Storage and transactions

Storage is an append-only, CRC32C-checksummed segment log with a manifest, using MVCC
version chains (storage format v2): each record id maps to an ordered chain of
commit-timestamped versions, and a delete is a tombstone version. On open the engine replays
segments to rebuild version chains and indexes; a torn tail is detected by checksum and
truncated. Transactions pin a read timestamp at `begin`, overlay their own staged writes,
and commit with optimistic first-committer-wins conflict detection. AuraDB implements
single-node **snapshot isolation** — not serializable isolation. See
[`docs/STORAGE_ENGINE.md`](docs/STORAGE_ENGINE.md) and
[`docs/TRANSACTIONS.md`](docs/TRANSACTIONS.md).

## Operations

- **Security.** Enforced static-token authentication (Argon2id-hashed, constant-time) and
  server-terminated TLS with optional mutual TLS (rustls), both fail-closed. In-place token
  rotation with `auradb auth rotate-token`. `#![forbid(unsafe_code)]` across every crate.
  Not implemented: RBAC, field-level encryption, encryption at rest, audit logging. See
  [`docs/SECURITY.md`](docs/SECURITY.md) and [`SECURITY.md`](SECURITY.md).
- **Backup / restore / upgrade.** `auradb dump` → `auradb backup verify` → `auradb restore`
  → `auradb check`; older data directories migrate to storage format v2 transparently on
  first open. See [`docs/OPERATIONS.md`](docs/OPERATIONS.md) and
  [`docs/UPGRADING.md`](docs/UPGRADING.md).
- **Health.** `auradb doctor` and `auradb status` print redacted health (`--json`);
  `auradb check` verifies on-disk index consistency.
- **Observability.** A metrics registry exports counters, gauges, and latency histograms as
  JSON and Prometheus text; structured tracing and health/readiness surfaces are built in,
  with no external collector required ([`docs/OBSERVABILITY.md`](docs/OBSERVABILITY.md)).
- **Runbooks.** Single-node production guidance and operator procedures are in
  [`docs/PRODUCTION_READINESS.md`](docs/PRODUCTION_READINESS.md) and
  [`docs/RUNBOOKS.md`](docs/RUNBOOKS.md).

A full command reference is in [`docs/CLI.md`](docs/CLI.md).

## Multi-node preview

AuraDB can form a static, cross-process cluster that elects a leader and replicates writes
through Raft over an authenticated, frame-checked peer transport. The preview is **off by
default** and gated behind two explicit `[cluster]` opt-ins:

```toml
[cluster]
enabled = true
experimental_multi_node = true
```

**What works:** leader election, majority-commit replication, leader-only writes (followers
return a structured `not_leader` error with a leader hint), follower catch-up, snapshot
install, and live tooling (`auradb cluster leader|wait-leader|wait-ready`, `auradb status
--json`). The validated path is the three-node loopback example in
[`examples/cluster`](examples/cluster).

**What remains preview:** this is for local testing and early validation only. It is **not
production HA**. Not provided: production automatic failover, dynamic membership,
linearizable follower reads, distributed transactions, sharding, and multi-region. The
evidence required before any production HA claim is tracked in
[`docs/HA_RELEASE_CANDIDATE.md`](docs/HA_RELEASE_CANDIDATE.md); see also
[`docs/CLUSTERING.md`](docs/CLUSTERING.md) and
[`docs/CLUSTER_TROUBLESHOOTING.md`](docs/CLUSTER_TROUBLESHOOTING.md).

## Not supported

AuraDB is deliberately honest about its boundaries. The following are **not** implemented and
**not** claimed: production HA, production automatic failover, production cluster readiness,
dynamic membership, distributed transactions, linearizable follower reads, sharding,
multi-region; approximate (ANN/HNSW) vector search; serializable isolation; RBAC, field-level
encryption, encryption at rest, and audit logging. Planned directions are in
[`docs/ROADMAP.md`](docs/ROADMAP.md).

## Documentation

**Getting started** — [Deployment](docs/DEPLOYMENT.md) · [CLI](docs/CLI.md) ·
[Configuration](docs/CONFIGURATION.md) · [Architecture](docs/ARCHITECTURE.md) ·
[Design decisions](docs/DECISIONS.md)

**Query & search** — [Query engine](docs/QUERY_ENGINE.md) ·
[Search & ranking](docs/SEARCH_AND_RANKING.md) · [Full-text](docs/FULL_TEXT.md) ·
[Vectors](docs/VECTORS.md) · [Documents](docs/DOCUMENTS.md) ·
[Relationships](docs/RELATIONSHIPS.md) · [Cursors](docs/CURSORS.md) ·
[Indexing](docs/INDEXING.md) · [Storage engine](docs/STORAGE_ENGINE.md) ·
[Transactions](docs/TRANSACTIONS.md)

**Operations** — [Production readiness](docs/PRODUCTION_READINESS.md) ·
[Operations](docs/OPERATIONS.md) · [Runbooks](docs/RUNBOOKS.md) ·
[Observability](docs/OBSERVABILITY.md) · [Upgrading](docs/UPGRADING.md) ·
[Benchmarks](docs/BENCHMARKS.md)

**Security** — [Security policy](SECURITY.md) · [Security model](docs/SECURITY.md)

**Compatibility** — [Compatibility matrix](docs/COMPATIBILITY.md) ·
[Support policy](docs/SUPPORT_POLICY.md) ·
[Connector compatibility](docs/AURA_CONNECTOR_COMPATIBILITY.md) ·
[Protocol](docs/PROTOCOL.md)

**Multi-node preview** — [HA candidate](docs/HA_RELEASE_CANDIDATE.md) ·
[Clustering](docs/CLUSTERING.md) · [Raft](docs/RAFT.md) ·
[Replication](docs/REPLICATION.md) · [Cluster troubleshooting](docs/CLUSTER_TROUBLESHOOTING.md)

**Release & contributing** — [Changelog](CHANGELOG.md) · [Roadmap](docs/ROADMAP.md) ·
[Release process](docs/RELEASE.md) · [Testing](docs/TESTING.md) ·
[Conformance](docs/CONFORMANCE.md) · [Contributing](CONTRIBUTING.md) ·
[Code of Conduct](CODE_OF_CONDUCT.md)

## Testing

```bash
cargo test --workspace --all-features
```

Tests span unit, integration over real TCP, backup/restore, the v1-to-v2 MVCC upgrade,
snapshot isolation and version GC, planner and `EXPLAIN ANALYZE`, ranked and hybrid search,
deterministic chaos/recovery and corruption drills, multi-node replication, and conformance.
See [`docs/TESTING.md`](docs/TESTING.md).

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
