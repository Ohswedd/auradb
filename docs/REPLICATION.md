# Replication

> **AuraDB v0.4.1 hardens the Raft groundwork introduced in v0.4.0. Multi-node
> server deployment remains experimental and disabled by default. Single-node
> mode remains the recommended production mode.** v0.4.1 strengthens apply
> idempotency under restart and the snapshot restore boundary (see *Snapshot
> boundary* below).

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
majority, so a proposed entry is committed immediately. When cluster mode is
disabled (the default), no replicated log is attached and commits go straight to
storage exactly as in v0.3.1.

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

## The snapshot boundary

This release does **not** ship streaming snapshot transfer between nodes. What it
does ship is the **boundary**: a versioned snapshot manifest that names the log
index a snapshot covers and carries a content digest, plus the create/restore seam
that captures and rebuilds engine state. Defining this now means a later release
can add over-the-wire snapshot shipping without another on-disk format change.

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

- No multi-node *server* replication: cross-process transport is not implemented,
  and configuring peers is rejected at startup. Multi-node consensus and the
  replicated apply path are validated by deterministic in-process tests only. See
  [CLUSTERING.md](CLUSTERING.md) and [RAFT.md](RAFT.md).
- No streaming snapshot shipping between nodes (only the snapshot boundary is
  defined).
- No distributed transactions. Cluster mode orders commits through Raft but does
  not change single-node isolation semantics; see [TRANSACTIONS.md](TRANSACTIONS.md).
