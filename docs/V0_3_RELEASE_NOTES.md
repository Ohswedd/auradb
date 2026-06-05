# AuraDB v0.3.0 release notes

**MVCC and query planner foundations.**

AuraDB v0.3.0 introduces multi-version concurrency control (MVCC) and a
cost-based query planner. Transactions now read from a consistent snapshot
pinned when they begin, and read queries are planned against persisted
statistics before execution. The release preserves all v0.2.1 behavior for
non-transactional reads and remains compatible with Aura Connector 0.3.x.

> AuraDB v0.3.0 implements single-node snapshot isolation with optimistic write
> conflict detection. It is not serializable isolation.

## Highlights

### Snapshot isolation (MVCC)

- Each record now keeps an ordered chain of committed versions; a delete is a
  committed tombstone version. Versions and tombstones survive restart.
- A transaction pins a **read timestamp** at `begin`. All of its reads — point
  lookups, scans, filters, counts, existence checks, vector search,
  document-path and full-text queries, relationship includes, and cursor paging
  — observe committed state as of that snapshot, overlaid with the transaction's
  own staged writes. A transaction never sees another transaction's later commit.
- Non-transactional reads continue to see the latest committed state, exactly as
  in v0.2.1.
- Commit uses optimistic, first-committer-wins write-conflict detection: if a
  record the transaction wrote was modified by a transaction that committed after
  the snapshot was pinned, commit fails with a conflict error (covering
  write-write, update-delete, and delete-update conflicts).

### Version garbage collection

- `auradb gc` reclaims versions that no active transaction can observe and drops
  fully-deleted records, always retaining the latest version and at least the
  configured minimum.
- A server can run GC in the background; configure it under `[mvcc]`
  (`gc_enabled`, `gc_interval_secs`, `min_retained_versions`).

### Query planner and statistics

- Read queries route through a cost-based planner that builds a plan tree (point
  lookup, secondary / document-path / full-text index lookup, vector search, full
  scan, and the filter, sort, offset, limit, projection, and relationship-include
  operators).
- The planner chooses an access path by estimated cost, using collection row
  counts and per-field cardinality. It prefers the most selective available index
  and falls back to a full scan when no index applies.
- `auradb stats analyze` computes and persists planner statistics
  (`planner_stats.json`); `auradb stats show` prints them. Row counts are kept
  current on every mutation; cardinality is refreshed by `analyze` and on
  compaction.

### EXPLAIN ANALYZE

- `EXPLAIN ANALYZE` executes a query and reports measured metrics alongside the
  plan: scanned, matched, and returned rows; execution and planning time; the
  index used; and the snapshot timestamp when run inside a transaction.
- It is requested as an optional flag carried in the raw Query IR, so it requires
  no protocol break and works with existing connectors.

## Upgrading

The on-disk storage format moves from v1 to v2 (commit-timestamped version
chains). A v0.1.0, v0.2.0, or v0.2.1 data directory is migrated to v2
transparently the first time v0.3.0 opens it; the upgrade is covered by tests
that run against real release fixtures. A data directory written by an unknown
future format is still rejected rather than silently opened. Back up your data
directory before upgrading, as with any release. See `docs/UPGRADING.md`.

## Compatibility

- **Aura Connector:** 0.3.x remains fully compatible with AuraDB 0.3.0. No
  connector release is required; `EXPLAIN ANALYZE` is reachable today through the
  raw Query IR. See `docs/AURA_CONNECTOR_COMPATIBILITY.md`.
- Authentication, TLS, persisted indexes, backup/restore, Docker images, and the
  binary release workflow are unchanged from v0.2.1.

## Not in this release

This release is single-node and does not add clustering, Raft, replication,
sharding, failover, or distributed transactions. It does not implement
serializable isolation, approximate nearest-neighbour indexes (HNSW/ANN), BM25,
or hybrid search fusion.
