# Roadmap

This roadmap describes where AuraDB is headed beyond the first single-node
release. It is a statement of direction, not a delivery commitment. Items are
grouped by theme and listed roughly in the order we expect to approach them.

## Current release: 0.6.1

AuraDB 0.6.1 **hardens snapshot install and published-cluster smoke** for the
controlled multi-node preview. It adds multi-architecture Docker images
(`linux/amd64` + `linux/arm64`), larger and concurrent-write snapshot-install
validation, snapshot-needed and follower-lag diagnostics (per-peer
`lag_entries` / `needs_snapshot` / `catch_up_state`, cluster snapshot
diagnostics, and a live `auradb cluster doctor --addr`), additional
snapshot-install metrics, cluster backup/restore **dry-run** planning
(`auradb cluster backup-plan` / `restore-plan`), and a published-image cluster
smoke checklist. It is **not production HA** and makes **no new
production-clustering claims** — there is no production automatic failover,
linearizable follower read, distributed transaction, dynamic membership,
sharding, or multi-region. There is **no format or wire change** (storage
unchanged; AWP 1 with additive snapshot/lag diagnostics fields), so Aura
Connector 0.3.x remains compatible and no connector release is required.
**Single-node mode remains the recommended production mode.**

### Delivered in 0.6.0

AuraDB 0.6.0 **improved the controlled multi-node preview and validated
fail-stop recovery.** It adds a leader kill / automatic re-election preview (a
stopped leader is taken over by the surviving majority; the old node rejoins as a
follower and catches up), the first real **peer snapshot install over the wire**
(a bounded single-message transfer for a follower that fell behind the compacted
prefix, with full validation and failure safety), larger follower catch-up
coverage, sharper fail-stop diagnostics (leader-change and snapshot-install
counters), a published-image Docker Compose smoke (`AURADB_IMAGE`), and peer
certificate/token rotation and cluster backup/restore runbooks. It is **not
production HA**: leader kill and re-election are a **fail-stop recovery preview**,
not production automatic failover, and there is no linearizable follower read,
distributed transaction, dynamic membership, sharding, or multi-region. There is
**no format or wire change** (storage unchanged; AWP 1 with additive fail-stop
diagnostics fields), so Aura Connector 0.3.x remains compatible and no connector
release is required. **Single-node mode remains the recommended production
mode.** The "not part of the preview" list below is unchanged — v0.6.0 makes no
new production-clustering claims.

### Delivered in 0.5.0

AuraDB 0.5.0 introduced a **controlled, experimental multi-node server preview**.
Real AuraDB server processes can now form a cross-process cluster, elect a leader,
and replicate writes through Raft over a dedicated, frame-checked, authenticated
peer transport. **Single-node mode remains the recommended production mode**, and
the preview is **off by default**, gated behind two explicit `[cluster]` opt-ins
(`enabled = true` and `experimental_multi_node = true`) with fail-closed
guardrails (a non-empty `peers` list requires the second opt-in; any non-loopback
address requires `allow_experimental_public_cluster = true` plus peer TLS and a
token). Membership is static. The leader write path blocks until a majority
commits; followers reject writes with `not_leader` and reject reads. The release
adds live cluster CLI commands (`auradb cluster leader|wait-leader|wait-ready`),
additive per-peer status fields (`preview_multi_node`, `quorum_available`,
`peers`), and multi-node metrics. There is **no format or wire change** (storage
unchanged from v0.4.x; AWP 1 with additive fields), so Aura Connector 0.3.x
remains compatible — a connector targets the leader's client address.

Still **not** part of the preview (and tracked as future): production multi-node
clustering, automatic failover, dynamic membership (join/leave), streaming
snapshot install over the wire (answered as unsupported), linearizable and
follower reads, sharding, and multi-region.

### Hardened in 0.4.1 (groundwork before the preview)

AuraDB 0.4.1 was a patch release that **hardened the Raft and replication
groundwork** shipped in 0.4.0: Raft log compaction boundaries, snapshot restore
edge cases, apply idempotency under restart, cluster-metadata corruption handling,
stronger peer configuration validation, deterministic multi-node partition tests,
single-node cluster overhead benchmarks, and operational diagnostics (`auradb
cluster compact-log`, `auradb snapshot create|inspect|restore`). The groundwork
itself was delivered in 0.4.0:

AuraDB 0.4.0 adds the **Replication and Raft groundwork**: an optional cluster
mode built on a durable, deterministic Raft consensus core, a replicated command
model that orders commits through the Raft log, and a versioned snapshot boundary.
The recommended production path remains **single-node, non-cluster mode**, which
is the default and is byte-for-byte unchanged from v0.3.1. When cluster mode is
enabled with no peers, the node runs as a real, durable single-node cluster (which
provides no fault tolerance). Multi-node server deployment is **experimental and
not enabled** in this release — configuring peers is rejected at startup, and the
consensus core is validated through deterministic in-process tests. The Aura Wire
Protocol is unchanged at AWP 1 (the cluster health field and `not_leader` error
code are additive), so Aura Connector 0.3.x remains fully compatible.

## Delivered in 0.4.0

- `auradb-cluster`: durable node/cluster identity (`node.json` / `cluster.json`,
  versioned and fail-closed on unknown future formats), the `[cluster]` config
  table, node role, and cluster status.
- `auradb-raft`: a durable, CRC32-checksummed Raft log (no-gap / no-term-regression
  invariants, torn-tail truncation, fail-closed on corruption) and a tick-driven,
  deterministic consensus state machine (election, replication, log repair, commit
  advancement) with a deterministic in-process multi-node simulation harness.
- `auradb-replication`: the `ReplicatedCommand` model and its versioned encoding,
  an idempotent apply path (the MVCC commit timestamp equals the Raft log index),
  restart replay of committed-but-unapplied entries, and a versioned snapshot
  boundary (create/restore of a portable logical dump).
- Engine integration: an optional replicated log attached in cluster mode; the
  default (disabled) write path is unchanged. Leader-only writes with an additive
  `not_leader` error code.
- Server: a `[cluster]` config table (disabled by default; single-node cluster when
  enabled with no peers; peers rejected at startup; non-loopback cluster bind
  rejected without `--allow-insecure-bind`) and an additive `cluster` health
  section.
- CLI: `auradb cluster init|status|peers|doctor|bootstrap`, plus cluster fields in
  `auradb status --json` and `auradb doctor`.
- Prometheus Raft/replication metrics.

See [CLUSTERING.md](CLUSTERING.md), [RAFT.md](RAFT.md), and
[REPLICATION.md](REPLICATION.md).

## Previous release: 0.3.1

AuraDB 0.3.1 is a stabilization release for the MVCC and planner behavior shipped
in 0.3.0. It hardens the transaction lifecycle (an active transaction registry,
transaction timeouts, and an abandoned-transaction reaper so a long-lived or
abandoned transaction can no longer pin versions forever without visibility),
strengthens garbage-collection validation, and surfaces MVCC pressure through
metrics, `status`, and `doctor` warnings. It preserves all 0.3.0 behavior, remains
compatible with Aura Connector 0.3.x, and prepares — but does not implement — the
codebase for future replication and Raft work.

## Delivered in 0.3.1

- Active transaction registry; transaction timeout and abandoned-transaction
  reaper with a structured `transaction_timeout` error.
- Stronger MVCC GC validation; `auradb gc --dry-run` / `--json` and a
  `bytes_reclaimed` GC report field.
- MVCC pressure metrics, an `mvcc` section in health/`status`, and `doctor`
  operational warnings.
- Upgrade safety tests across genuine v0.1.0/v0.2.0/v0.2.1/v0.3.0 fixtures, planner
  regression tests, backup/restore-with-GC tests, and `auradb bench compare`.
- Richer `EXPLAIN ANALYZE` diagnostics (additive JSON fields).

## Previous release: 0.3.0

AuraDB 0.3.0 adds MVCC and query planner foundations on top of the 0.2.x
single-node release: each record keeps a chain of committed versions, transactions
read from a snapshot pinned at `begin` (single-node snapshot isolation with
optimistic write conflict detection), version garbage collection reclaims old
versions, and read queries route through a cost-based planner with persisted
statistics and `EXPLAIN ANALYZE`. The on-disk storage format moves to v2 and an
older directory is migrated transparently on first open. It preserves all v0.2.1
behavior for non-transactional reads and remains compatible with Aura Connector
0.3.x (no connector release required).

The carried-forward 0.2.x feature surface is a single-node database focused on
security, durability hardening, and public usability: persistent storage,
transactions, a typed schema catalog, the Query IR executor, primary, unique,
secondary, document-path, full-text, and exact vector indexes, document fields,
relationship includes, server-side cursors, observability, a CLI, and Docker
support. See the [CHANGELOG](../CHANGELOG.md) for the full feature list and
[README](../README.md) for what is and is not claimed.

## Delivered in 0.3.0

- MVCC storage with commit-timestamped version chains and tombstones (storage
  format v2, with transparent v1-to-v2 migration on first open).
- Single-node snapshot isolation: transactions pin a read timestamp at `begin` and
  read from that snapshot, with optimistic first-committer-wins write-conflict
  detection. This is not serializable isolation.
- Version garbage collection (`auradb gc` and optional background GC) that
  reclaims versions no active transaction can observe.
- A cost-based query planner with a plan tree and costed index selection driven by
  collection row counts and per-field cardinality.
- Persisted planner statistics (`planner_stats.json`), refreshed by `auradb stats
  analyze` and on compaction, with row counts kept current on each mutation.
- `EXPLAIN ANALYZE` with execution metrics, reachable through the raw Query IR
  with no protocol break.

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

MVCC version chains and single-node snapshot isolation with optimistic write
conflict detection ship in 0.3.0 (see *Delivered in 0.3.0*). Future direction:

- Serializable isolation (the current release is snapshot isolation, which does
  not prevent write-skew).
- Configurable isolation levels.

## Security

- Role-based access control (RBAC) on top of the existing enforced
  authentication.
- Field-level encryption, encryption at rest, and an audit log.

Enforced TLS and enforced static-token authentication ship in 0.2.0.

## Distribution

The Raft and replication **groundwork** shipped in 0.4.0 (a durable consensus
core, a replicated commit path, single-node cluster mode, and a snapshot
boundary). **Preview (0.5.0):** a controlled, experimental cross-process
multi-node server preview — real leader election, AppendEntries replication,
majority commit, follower apply, and follower catch-up over an authenticated peer
transport. It is off by default, gated behind two opt-ins, with static membership.
Single-node mode remains the recommended production path.

The following remain **future** and are not present in 0.5.0:

- **Production** multi-node clustering and production-grade peer networking (the
  0.5.0 multi-node path is an experimental preview).
- Automatic failover.
- Dynamic cluster membership / joint consensus (`join` / `leave` / `step-down`);
  membership is static in the preview.
- **Chunked / streaming** snapshot install between nodes. v0.6.0 ships a
  **bounded, single-message** peer snapshot install (capped at 8 MiB); chunked
  streaming of arbitrarily large snapshots is still future work.
- Linearizable reads and follower reads (followers reject reads in the preview).
- Sharding and multi-region deployment.

These remain explicitly out of scope for the multi-node preview and are not
implied by any current documentation.

## Data services

- Change streams for downstream consumers.
- Time-travel queries over historical versions.

## Ecosystem

- Pin golden Aura Wire Protocol frame and Query IR fixtures from the published
  Aura Connector package and add them to the conformance suite.
- Expanded language client support.
