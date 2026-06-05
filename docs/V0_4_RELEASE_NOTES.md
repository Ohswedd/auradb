# AuraDB v0.4.0 release notes

**The replication and Raft foundation for future clustered deployments.**

AuraDB v0.4.0 begins AuraDB's distributed architecture. It is a conservative,
honest first step: it introduces a correct, durable, testable cluster foundation
— stable identity, a durable Raft log, a deterministic Raft state machine, a
leader-only write path, an idempotent replicated apply path, a snapshot boundary,
and cluster tooling and metrics. It does **not** turn AuraDB into a production
distributed database.

> **Single-node mode remains the recommended production path.** When cluster mode
> is disabled — the default — every v0.3.1 behavior is preserved byte-for-byte.
> Multi-node clustering is **experimental**: the Raft and replication core is
> validated by deterministic in-process tests, but cross-process multi-node
> server deployment is not enabled in this release (configuring peers is rejected
> at startup), and a single-node cluster provides no fault tolerance.
>
> This release does **not** add: production-grade multi-node clustering,
> automatic failover, linearizable reads, distributed transactions, sharding, or
> multi-region. It also adds no ANN/HNSW vector indexes and no BM25 ranking.

## Why this release

AuraDB has been a complete, honest single-node engine. v0.4.0 lays the
groundwork for replication without overclaiming: it builds and tests the hard,
correctness-critical pieces of consensus (a durable log, elections, log
replication and repair, commit advancement, idempotent apply, crash recovery) in
isolation, wires a real single-node Raft deployment into the server, and is
explicit about what is and is not ready for multi-node production use.

## Highlights

### Stable cluster identity

- `auradb init` now creates a persistent **node id** and **cluster id**, stored
  under `<data_dir>/cluster/` (`node.json`, `cluster.json`). Identity survives
  restarts; a config-pinned id that conflicts with persisted identity fails
  closed. Unknown future metadata formats are rejected rather than guessed at.

### Durable Raft log and deterministic state machine

- A checksummed, append-only **Raft log** (`raft-log.bin`) with crash-safe
  recovery: a torn trailing entry is truncated on open, a checksum mismatch fails
  closed, and appends enforce no index gaps and no term regressions.
- A minimal, correct **Raft state machine** — follower, candidate, and leader
  roles; `RequestVote` and `AppendEntries`; heartbeats; log consistency checks
  and repair; and commit-index advancement — driven by a **logical test clock**
  so behavior is reproducible and never timing-flaky. Multi-node consensus is
  exercised through a deterministic in-process simulation (leader election, log
  replication, follower catch-up, and a partition scenario).

### Single-node Raft mode

- Enable cluster mode with `[cluster] enabled = true` (no peers). Every
  data-plane write is then ordered through the durable local Raft log and applied
  to storage at the committed log index, which doubles as the MVCC commit
  timestamp — so ordering is deterministic and apply is idempotent. On restart,
  committed-but-unapplied entries are replayed. MVCC commit order, index
  consistency, and planner statistics are all preserved through the Raft path.

### Leader-only writes and the `not_leader` error

- A leader-and-follower role model with a leader-only write path. A new protocol
  error code, `not_leader`, lets a follower reject writes with a leader hint. In a
  single-node cluster the node is always the leader. The Aura Wire Protocol
  version is unchanged.

### Snapshot boundary

- A versioned **snapshot manifest** (with a content digest) captures and restores
  engine state, establishing the boundary for future over-the-wire state
  transfer. Streaming snapshot shipping between nodes is not part of this release.

### Tooling, metrics, and status

- New CLI: `auradb cluster init|status|peers|doctor|bootstrap`. Membership
  commands (`join`, `leave`, `step-down`) are intentionally not provided because
  membership changes are not implemented.
- `auradb status --json`, `auradb doctor`, and the server health report include a
  cluster section (node id, cluster id, role, term, leader id, commit/applied/
  last-log index, peer count, replication lag).
- Prometheus metrics for cluster and replication state: `auradb_cluster_enabled`,
  `auradb_node_role`, `auradb_raft_current_term`, `auradb_raft_commit_index`,
  `auradb_raft_applied_index`, `auradb_raft_log_last_index`,
  `auradb_raft_leader_changes_total`, `auradb_raft_votes_granted_total`,
  `auradb_raft_append_entries_sent_total`,
  `auradb_raft_append_entries_received_total`,
  `auradb_raft_replication_lag_entries`, `auradb_replication_apply_errors_total`,
  and `auradb_raft_apply_latency_us`.

## Compatibility

- **Aura Connector 0.3.x remains fully compatible. No connector release is
  required.** The cluster health section and the `not_leader` error code are
  additive: a 0.3.x connector ignores unknown JSON fields and maps unknown error
  codes safely.
- **Storage format is unchanged.** A v0.3.1 data directory opens directly.
  Cluster metadata is created only when you enable cluster mode (or run
  `auradb cluster init`); existing data is never forced into cluster mode.
- **Aura Wire Protocol is unchanged at version 1.**

## Upgrading

v0.4.0 is a drop-in binary replacement for v0.3.1. Stop the old server, install
v0.4.0, and start it against the same data directory. To try single-node cluster
mode on existing data, enable `[cluster]` in your config (see
`examples/auradb.cluster.local.toml`). See [`docs/UPGRADING.md`](UPGRADING.md).

## Limitations

- A **single-node cluster has no fault tolerance** — it is the same availability
  as non-cluster single-node mode, with additional write-path overhead. Use it to
  exercise the replication path, not for high availability.
- **Multi-node server deployment is not enabled.** Configuring `peers` is rejected
  at startup. Cross-process cluster transport and its authentication story are not
  production-ready in this release; cluster traffic is loopback-only.
- **No membership changes, no automatic failover, no linearizable reads, no
  distributed transactions, no sharding, no multi-region.**
- Reads are served by the leader; AuraDB does not claim linearizable follower
  reads.

See [`docs/CLUSTERING.md`](CLUSTERING.md), [`docs/RAFT.md`](RAFT.md), and
[`docs/REPLICATION.md`](REPLICATION.md) for details.
