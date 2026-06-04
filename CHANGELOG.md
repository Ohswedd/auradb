# Changelog

All notable changes to AuraDB are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project uses
[Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-06-04

First single-node developer release.

### Added

- **Storage engine.** Append-only, checksummed segment log with a manifest,
  crash recovery (torn-tail truncation, corruption detection), and compaction.
- **Aura Wire Protocol.** Binary framed protocol with version negotiation,
  header and payload CRC32 checksums, request-id correlation, and structured
  error frames.
- **Transactions.** Buffered write sets with optimistic write and read conflict
  detection, atomic durable commit, and rollback.
- **Schema catalog.** Typed fields, primary keys, unique and secondary indexes,
  document and vector fields, relationships, and validation.
- **Query engine.** Find, filter (comparisons, `contains`, `AND`/`OR`/`NOT`),
  order/limit/offset, projection, count, exists, insert, bulk insert, update,
  delete, upsert, relationship includes, document path access, exact vector
  nearest-neighbour search, and EXPLAIN.
- **Migration impact estimation.**
- **Server-side cursors** with paging and idle-timeout reaping.
- **Server.** Async TCP listener, concurrent connections, payload limits,
  graceful shutdown, and per-connection transactions.
- **Observability.** Metrics registry (counters, gauges, latency histograms)
  with JSON and Prometheus-text export, plus structured tracing.
- **CLI.** `version`, `init`, `server`, `doctor`, `status`, `check`, `compact`,
  `dump`, `restore`, `bench`.
- **Conformance harness.** A protocol client and scenario suite, plus a Python
  harness.
- Docker support, example configuration, benchmarks, and GitHub Actions CI.

### Not yet implemented (not claimed)

Distributed clustering, replication, sharding, failover, multi-region, and Raft;
approximate (ANN/HNSW) vector indexes; BM25 full-text and hybrid fusion ranking;
serializable MVCC; enforced TLS and authentication; field-level encryption,
RBAC; time travel; and change streams. See [docs/ROADMAP.md](docs/ROADMAP.md).

[0.1.0]: https://github.com/Ohswedd/auradb/releases/tag/v0.1.0
