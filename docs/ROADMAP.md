# Roadmap

This roadmap describes where AuraDB is headed beyond the first single-node
release. It is a statement of direction, not a delivery commitment. Items are
grouped by theme and listed roughly in the order we expect to approach them.

## Current release: 0.1.0

AuraDB 0.1.0 is the first single-node developer release. It provides persistent
storage, transactions, a typed schema catalog, the Query IR executor, primary,
unique, secondary, and exact vector indexes, document fields, relationship
includes, server-side cursors, observability, a CLI, and Docker support. See the
[CHANGELOG](../CHANGELOG.md) for the full feature list and
[README](../README.md) for what is and is not claimed.

## Vector search

- Approximate nearest-neighbour indexing (for example HNSW) behind the existing
  `VectorIndex` trait, with recall and persistence tests. Exact search remains
  the correctness baseline.
- Quantization options for large embedding sets.

## Text and hybrid search

- BM25 full-text indexing.
- Hybrid fusion ranking that blends vector similarity and text relevance.

## Indexing and storage

- Persisted secondary index files so indexes are not rebuilt on open.
- Document path indexes for nested field lookups.
- Background compaction tuning and segment lifecycle controls.

## Transactions and consistency

- Promotion from snapshot reads with optimistic conflict detection toward
  serializable MVCC with version chains.
- Configurable isolation levels.

## Security

- Enforced TLS for client connections.
- Enforced authentication, then role-based access control (RBAC).
- Field-level encryption and an audit log.

## Distribution

- Replication and failover.
- Clustering and sharding with a consensus protocol such as Raft.
- Multi-region deployment.

These distributed capabilities are explicitly not present in 0.1.0 and are not
implied by any current documentation.

## Data services

- Change streams for downstream consumers.
- Time-travel queries over historical versions.

## Ecosystem

- Pin golden Aura Wire Protocol frame and Query IR fixtures from the published
  `aura-connector` package and add them to the conformance suite.
- Expanded language client support.
