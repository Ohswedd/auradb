# Replication

> **AuraDB v0.9.0 is an HA release candidate for the controlled static-cluster
> preview, not a production HA guarantee. Single-node mode remains the
> recommended production mode.** v0.9.0 strengthens snapshot/compaction coverage
> (larger installs, compaction with an offline follower, indexed-workload
> preservation, safe-to-retry install failures, snapshot metrics) without
> changing replication semantics or the storage format (v2). See
> [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) and
> [V0_9_RELEASE_NOTES.md](V0_9_RELEASE_NOTES.md).

> **AuraDB v0.6.0 improves the controlled multi-node preview and validates
> fail-stop recovery. It is _not_ production HA. Single-node mode remains the
> recommended production mode.** v0.6.0 adds the first real **peer snapshot
> install over the wire**: a follower that has fallen behind the leader's
> compacted prefix is brought current by a bounded, single-message snapshot
> (validated for cluster id, format, digest, boundary, storage format, and size),
> then resumes AppendEntries — replacing the v0.5.x structured *unsupported*
> response. It also adds a leader kill / re-election preview and larger follower
> catch-up coverage (preview behavior, not production automatic failover). Building
> on the v0.4.x replication groundwork (apply idempotency under restart, the
> snapshot restore boundary), v0.5.0 replicates writes across real server
> processes: the leader commits on a majority, followers apply committed entries,
> and a restarted follower catches up. The preview is off by default.

AuraDB maps database mutations onto the replicated Raft log and applies committed
entries back to the engine. This document describes the replicated command model,
the leader-only write path, the idempotent apply path, restart replay, the
snapshot boundary, and the replication metrics.

The replication layer is the bridge between the Raft core (consensus over an
opaque log, see [RAFT.md](RAFT.md)) and the engine (which holds the data). The
Raft core never has to understand storage batches or schemas; the replication
layer owns the payload format and its own versioning.

## The replicated command model

A replicated command is the database-level meaning carried inside a Raft
command's opaque payload. There are three kinds:

- **Noop** — mirrors a Raft leader no-op; applies to nothing.
- **Write(batch)** — a committed data-plane write batch.
- **Schema** — a schema change: create/replace a collection schema, or drop a
  collection.

Each command is encoded into a framed, versioned envelope (`ENVELOPE_VERSION = 1`)
and placed in a Raft command tagged with the matching command kind. Decoding
**rejects an envelope with a newer version** rather than misreading it, so a
future format is detected and fails closed. A bare Raft no-op (a leader's term
anchor) carries no payload and decodes to `Noop`.

## The leader-only write path

When cluster mode is enabled, the engine routes every data-plane commit through
an attached replicated log:

1. A write reaching a non-leader is rejected with `Error::NotLeader` (the
   `not_leader` wire error code), carrying a hint identifying the current leader
   when known.
2. On the leader, the batch is encoded as a `Write` command and proposed to the
   Raft log.
3. The leader returns the committed **log index**. The engine uses that index as
   the MVCC commit timestamp and applies the batch to storage inline.

In a single-node cluster the sole node is always the leader and is its own
majority, so a proposed entry is committed immediately. In the v0.5.0 multi-node
preview, the leader replicates the entry to its peers and the write path **blocks
until a majority commits** — a minority cannot commit, and a follower that
receives the write returns `not_leader` rather than accepting it. Followers also
reject reads by default. When cluster mode is disabled (the default), no
replicated log is attached and commits go straight to storage exactly as in
v0.3.1.

## The idempotent apply path

Applying a committed command is the same operation whether it runs on a leader
recovering after a crash or on a follower receiving the leader's committed
entries: decode the command, then route it to the engine's apply path.

The key invariant makes this safe to repeat:

> **The MVCC commit timestamp equals the Raft log index.**

Because the log index is deterministic across replicas and monotonic (gaps from
no-op entries are allowed), the apply is idempotent: the engine's replicated apply
is a no-op at or below its commit watermark. Replaying any prefix of the log is
therefore always safe — a write applied twice has no second effect.

- `Write` commands apply through the engine's idempotent batch apply, keyed by the
  log index.
- `Schema` commands create/replace or drop a collection.
- `Noop` commands apply to nothing.

## Restart replay

When the server starts a single-node cluster, the coordinator:

1. Opens the durable Raft log and campaigns — a single voter is its own majority,
   so it elects itself leader immediately.
2. Replays any entry that was committed to the Raft log but not yet applied to
   storage (between the engine's commit watermark and the Raft commit index).

This closes the crash window between a durable Raft commit and the storage apply,
so a write that reached the log is never lost on restart, and replaying it is safe
because apply is idempotent.

In the v0.5.0 multi-node preview the same path covers **follower catch-up after
restart**: a restarted follower replays its own durable log to its watermark and
the leader then brings it current over the peer transport via AppendEntries. The
idempotent apply (commit timestamp equals log index) means a follower never
double-applies an entry it already has.

## The snapshot boundary

The **boundary** is a versioned snapshot manifest that names the log index a
snapshot covers and carries a content digest, plus the create/restore seam that
captures and rebuilds engine state. v0.6.0 builds the first real **peer snapshot
install** on top of this boundary (see below); the boundary's on-disk format is
unchanged.

The snapshot manifest (`SNAPSHOT_FORMAT_VERSION = 1`) records:

- `last_included_index` and `last_included_term` — the log position the snapshot
  covers; entries at or below this index may be compacted once a snapshot is
  durable. This lines up with the Raft log's compacted prefix (see
  [RAFT.md](RAFT.md)).
- `cluster_id` / `node_id` (v0.4.1, optional) — the cluster and node the snapshot
  was taken from, so a restore can detect a cluster mismatch.
- `storage_format_version` (v0.4.1) — the storage format the snapshot was captured
  from; a restore into a build that cannot read it is refused.
- `collections` / `records` (v0.4.1) — captured counts, a quick integrity
  cross-check surfaced by `auradb snapshot inspect`.
- `digest` — a CRC32 digest of the payload, verified on read.
- `created_by_version` / `created_at_unix` — provenance.

The payload is a portable logical dump (schemas plus current live records)
captured through the engine's public API. A restore rebuilds storage, indexes,
and planner statistics into a fresh engine exactly as a normal load would.

**Restore hardening (v0.4.1).** `restore_to(dir, opts)` is **atomic**: it
materializes the snapshot into a staging directory beside the target, validates
it, and only then swaps it into place — a failure never corrupts existing data. It
refuses to overwrite a non-empty target unless `force` is set, and rejects a newer
format version, a storage format it cannot read, a cluster-id mismatch (unless
explicitly allowed), a corrupt manifest, and a payload digest mismatch **before
touching the target**. The v0.4.1 manifest fields are additive and optional, so a
v0.4.0 manifest still decodes. Operators drive this through `auradb snapshot
create|inspect|restore` (see [CLI.md](CLI.md)).

## Peer snapshot install over the wire (v0.6.0)

When a follower has fallen behind the leader's **compacted** prefix — its next
index is at or below the leader's last-included index, so AppendEntries can no
longer serve it — the leader ships a snapshot over the peer transport instead of
hanging:

1. **Detect.** The driver notices the lagging follower (rate-limited per peer so a
   still-installing follower is not flooded) and captures a manifest covering the
   leader's current committed state, tagged with the cluster and node id.
2. **Transfer.** The manifest is sent as a single `InstallSnapshotRequest`
   message, base64-framed and capped at `MAX_SNAPSHOT_BYTES` (8 MiB). This is a
   **bounded, single-message** transfer, not chunked streaming; a dataset whose
   snapshot exceeds the cap is logged and not shipped.
3. **Validate.** Before mutating any state, the follower checks the cluster id,
   manifest format version, payload digest, last-included index/term agreement,
   storage format version, and the size limit. Any failure is rejected and leaves
   existing follower state untouched.
4. **Install.** The follower installs the snapshot into its live engine at the
   snapshot's commit timestamp (`commit_ts_base + last_included_index`), advances
   its durable Raft boundary (discarding the subsumed prefix), and acknowledges.
5. **Resume.** The leader records the install (advancing its match/next index for
   that peer) and resumes AppendEntries from just past the boundary.

Live log compaction that creates this situation is **operator-initiated**
(`PeerCluster::compact_log`, the live counterpart to `auradb cluster
compact-log`); the driver does not compact the log out from under a healthy
follower on its own.

**Preview limitations.** This is a single-message bounded transfer, not chunked
streaming, and it targets the fail-stop case where the follower is strictly behind
the snapshot (it only fell behind) — it does not reconcile divergent follower
history. The counters `auradb_cluster_snapshots_sent_total`,
`auradb_cluster_snapshots_installed_total`, and
`auradb_cluster_snapshots_rejected_total` track the path. It is a preview, not a
production-grade snapshot subsystem.

## Larger-state and multi-model recovery (v0.6.2)

v0.6.2 validates recovery at larger sizes and across **all** field and index
kinds, over the **unchanged** v0.6.0/v0.6.1 replication and snapshot-install
paths. A follower is stopped while the majority commits a larger run of records
spanning scalar, secondary-indexed, full-text, document-path, and vector fields;
after it restarts it catches up by AppendEntries (or a snapshot install, when the
live majority has compacted past the entries it needs), and its counts, spot
reads, secondary index, full-text search, document-path queries, vector
nearest-neighbor results, and planner-used indexes are verified to match the rest
of the cluster — then re-verified after a full-cluster restart. The CI-safe size
is 120 records, with an `#[ignore]`d 5,000-record stress variant. Because MVCC
`commit_ts == Raft log index`, apply stays idempotent and monotonic across the
catch-up and snapshot boundaries, so there is **no duplicate apply** even under
concurrent leader writes. See [TESTING.md](TESTING.md).

## Snapshot install diagnostics (v0.6.1)

v0.6.1 makes the snapshot-install path observable; the wire transfer itself is
**unchanged from v0.6.0** (still a bounded, single-message transfer). The live
`auradb cluster status --addr <server>` report gains per-peer catch-up fields:

- `catch_up_state` — one of `normal`, `probing`, `snapshot_needed`,
  `snapshot_installing`, `caught_up`, or `unknown`.
- `lag_entries` — how far the follower's match index trails the leader.
- `needs_snapshot` / `snapshot_in_progress` — whether the follower has fallen
  below the compacted prefix (so AppendEntries can no longer serve it) and
  whether an install is currently running for that peer.

It also adds cluster-level snapshot diagnostics: the last installed boundary
(index/term), the last install time, the last error (the rejection reason), bytes
sent, bytes installed, an in-progress gauge, and a needed-total. The same signals
are exported as metrics (see [OBSERVABILITY.md](OBSERVABILITY.md)) and surfaced
by `auradb cluster doctor --addr` (see [CLI.md](CLI.md)).

Larger and concurrent snapshot-install scenarios are covered in
`crates/auradb-replication/tests/multi_node.rs` — a CI-safe larger run plus
`#[ignore]`d 1,000-entry and 10,000-entry stress runs — asserting that data, the
secondary index, planner statistics, and MVCC timestamps converge, with **no
duplicate apply** even under concurrent leader writes during the install.

## Replication metrics

When cluster mode is enabled, replication exposes Prometheus metrics (see
[OBSERVABILITY.md](OBSERVABILITY.md) for the full list and the health fields):

- `auradb_raft_current_term`, `auradb_raft_commit_index`,
  `auradb_raft_applied_index`, `auradb_raft_log_last_index`
- `auradb_raft_leader_changes_total`, `auradb_raft_votes_granted_total`
- `auradb_raft_append_entries_sent_total`,
  `auradb_raft_append_entries_received_total`
- `auradb_raft_replication_lag_entries` (committed minus applied)
- `auradb_replication_apply_errors_total`
- `auradb_raft_apply_latency_us` (apply latency summary)

`replication_lag_entries` is committed-minus-applied; in a single-node cluster it
is normally zero because each commit is applied inline.

## What this release does not do

- The multi-node *server* replication is an **experimental preview**, off by
  default and gated behind two opt-ins; it is **not production HA**, not
  production multi-node clustering, and provides no production automatic failover.
  See [CLUSTERING.md](CLUSTERING.md) and [RAFT.md](RAFT.md).
- Peer snapshot install (v0.6.0) is a **bounded, single-message** transfer, not
  chunked streaming, and targets the strictly-behind follower case.
- No follower reads or linearizable reads; followers reject reads by default.
- No distributed transactions. Cluster mode orders commits through Raft but does
  not change single-node isolation semantics; see [TRANSACTIONS.md](TRANSACTIONS.md).
