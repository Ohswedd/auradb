# Roadmap

This roadmap describes where AuraDB is headed beyond the first single-node
release. It is a statement of direction, not a delivery commitment. Items are
grouped by theme and listed roughly in the order we expect to approach them.

## Current release: 0.2.0

AuraDB 0.2.0 is a single-node release focused on security, durability hardening,
and public usability. It provides persistent storage, transactions, a typed
schema catalog, the Query IR executor, primary, unique, secondary, document-path,
full-text, and exact vector indexes, document fields, relationship includes,
server-side cursors, observability, a CLI, and Docker support. See the
[CHANGELOG](../CHANGELOG.md) for the full feature list and
[README](../README.md) for what is and is not claimed.

## Delivered in 0.2.0

These single-node hardening items are now delivered:

- Enforced static-token authentication (Argon2id-hashed, fail-closed).
- Server-terminated TLS with optional mutual TLS (rustls, fail-closed).
- Persisted index snapshots with fingerprint-based staleness detection and safe
  rebuild.
- Document-path indexes for nested-field equality lookups.
- Basic full-text search (tokenized boolean-AND matching with term-frequency
  ranking; not BM25).
- Richer conformance coverage (auth, TLS, document-path, and full-text scenarios).
- Deterministic seeded recovery and corruption fuzzing.
- A published Docker image (`ghcr.io/ohswedd/auradb`) and prebuilt binary release
  artifacts with `SHA256SUMS`.

## Vector search

- Approximate nearest-neighbour indexing (for example HNSW) behind the existing
  `VectorIndex` trait, with recall and persistence tests. Exact search remains
  the correctness baseline.
- Quantization options for large embedding sets.

## Text and hybrid search

- BM25 full-text ranking (the current full-text index uses term-frequency
  ranking, not BM25).
- Hybrid fusion ranking that blends vector similarity and text relevance.

## Indexing and storage

- Incremental index snapshots that avoid full rebuilds after a fingerprint
  mismatch (snapshots and safe rebuild already ship in 0.2.0).
- Background compaction tuning and segment lifecycle controls.

## Transactions and consistency

- Promotion from snapshot reads with optimistic conflict detection toward
  serializable MVCC with version chains.
- Configurable isolation levels.

## Security

- Role-based access control (RBAC) on top of the existing enforced
  authentication.
- Field-level encryption, encryption at rest, and an audit log.

Enforced TLS and enforced static-token authentication ship in 0.2.0.

## Distribution

- Replication and failover.
- Clustering and sharding with a consensus protocol such as Raft.
- Multi-region deployment.

These distributed capabilities are explicitly not present in 0.2.0, have not been
started, and are not implied by any current documentation.

## Data services

- Change streams for downstream consumers.
- Time-travel queries over historical versions.

## Ecosystem

- Pin golden Aura Wire Protocol frame and Query IR fixtures from the published
  Aura Connector package and add them to the conformance suite.
- Expanded language client support.
