# Raft Consensus Core

> **AuraDB v0.9.0 is an HA release candidate for the controlled static-cluster
> preview, not a production HA guarantee. Single-node mode remains the
> recommended production mode.** v0.9.0 strengthens repeated leader-change and
> snapshot-install testing (3-cycle CI restart, old-leader rejoin, no duplicate
> apply, index convergence, compaction with an offline follower) without changing
> the consensus core, the storage format (v2), or the wire protocol (AWP 1). See
> [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) and
> [V0_9_RELEASE_NOTES.md](V0_9_RELEASE_NOTES.md).

> **AuraDB v0.6.0 improves the controlled multi-node preview and validates
> fail-stop recovery. It is _not_ production HA. Single-node mode remains the
> recommended production mode.** Stopping the leader lets the surviving majority
> elect a new one, which accepts writes; the restarted old leader rejoins as a
> follower and catches up — by AppendEntries, or by a v0.6.0 **peer snapshot
> install** if it fell behind the compacted prefix. This is fail-stop recovery
> preview behavior, not production automatic failover. Building
> on the v0.4.x Raft groundwork (log compaction boundaries, durability checks,
> deterministic multi-node partition tests), v0.5.0 runs Raft over a real
> cross-process peer transport so server processes can elect a leader and
> replicate to one another. The preview is off by default.

AuraDB's Raft log and consensus core is a minimal, deterministic implementation
that underpins cluster mode. This document describes the log model, the durable
storage and its corruption handling, the node state machine, the deterministic
test clock, and precisely what is and is not implemented.

The Raft core is database-agnostic: it orders an opaque, framed log of commands
and never interprets their bytes. The database meaning of a command is owned by
the replication layer (see [REPLICATION.md](REPLICATION.md)).

## Terms and log indices

- A **term** is a logical clock that increases on every election. The zero term
  is a sentinel meaning "before the first entry".
- A **log index** is 1-based; index `0` means "before the first entry".

Every log entry is identified by a `(term, index)` pair.

## Entries, commands, and command kinds

A **log entry** carries its `term`, its `index`, and a **command**. A command is
a framed, versioned envelope:

- `version` — the command envelope version (current value `1`). A future version
  is detected and rejected rather than misread.
- `kind` — one of `Noop`, `Database`, `Schema`, `Metadata`, or `Config`. The kind
  keeps the log self-describing and lets the apply layer route a payload without
  decoding it.
- `payload` — opaque bytes owned by the replication layer.

A `Noop` command is a leader no-op used to anchor a new term's commit point.

## Hard state

The durable, crash-critical state that must survive restarts is the **hard
state**:

- `current_term` — the latest term this node has seen.
- `voted_for` — the candidate this node voted for in `current_term`, if any.
- `commit_index` — the highest log index known to be committed.

## Durable storage and corruption handling

Two storage backends implement the same interface:

- `MemStorage` — an in-memory log used for deterministic tests.
- `FileStorage` — a durable, file-backed log.

The file backend lays out two files in its directory (under `<data_dir>/cluster/`
in a running server):

```text
raft-log.bin      # append-only, framed, checksummed log entries
raft-state.json   # the hard state (current term, vote, commit index)
```

Each log frame is `[len: u64 BE][crc32: u32 BE][json bytes]`. This framing gives
the same durability discipline the storage engine uses for its segments:

- A **torn trailing frame** — a crash mid-append that leaves a partial frame at
  the end — is detected and truncated on open, so subsequent appends extend valid
  data.
- A **checksum mismatch** on a fully present frame fails closed as corruption
  rather than being read as valid data.

On open, the log is replayed, validating every frame's checksum and the log
invariants below. The hard state is written atomically (write-temp-then-rename).

### Log invariants

Appends always extend the tail and are validated on every append:

- **No gaps.** Entry indices must be contiguous.
- **No term regression.** Terms must never decrease along the log.

The consensus core truncates any conflicting suffix before appending, so an
append is always a clean extension of the tail.

## Log compaction boundaries (v0.4.1)

Once a snapshot durably covers a prefix of the log, those entries can be
discarded. The durable store (`FileStorage`) records the compacted prefix — the
**last included index** and **term** — in `raft-compaction.json`, and keeps only
the retained suffix in `raft-log.bin`. The boundary is enforced strictly:

- **Compaction never runs ahead of durability.** `compact(up_to, applied)`
  refuses (with `CompactionRefused`) to discard any entry that is beyond the
  committed index, beyond the applied index the caller supplies, or beyond the end
  of the log. `compactable_prefix(applied)` returns the highest safely compactable
  index — the minimum of the committed index, the applied index, and the last
  index.
- **The boundary is preserved.** After compaction, `last_included_index` /
  `last_included_term` are retained; `term_at(last_included_index)` still resolves
  the boundary term, so the AppendEntries consistency check works across the
  compacted prefix.
- **Reads before the prefix fail closed.** `read_at(index)` for an index at or
  below the compacted prefix returns a structured `Compacted` error rather than a
  wrong or empty result. Truncating into the compacted prefix is likewise refused.
- **Restart preserves the boundary, and corruption fails closed.** The compaction
  metadata is reloaded on open; a future format version, or metadata that
  disagrees with the retained log's first entry, is rejected as corruption.
- **Snapshots line up with the boundary.** A local snapshot is captured at a
  `last_included_index` / `last_included_term` that matches the compacted prefix
  (see [REPLICATION.md](REPLICATION.md)).

Operators compact through `auradb cluster compact-log [--dry-run] [--json]` (see
[CLI.md](CLI.md)). Compaction is local: this release does **not** ship streaming
snapshot transfer between nodes.

## The node state machine

A Raft node is a pure state machine driven by a **logical clock**. It performs no
I/O of its own beyond its storage and a queue of outgoing messages that the caller
delivers. A node is always exactly one role:

- **Follower** — accepts log entries from a leader and grants votes.
- **Candidate** — stands for election in the current term.
- **Leader** — the elected leader for the current term and the only node that
  accepts writes.

### Elections, heartbeats, and commit

- Each tick advances the logical clock by one unit. A follower or candidate that
  reaches its election timeout starts a new election (`campaign`), incrementing
  its term, voting for itself, and soliciting votes via `RequestVote`.
- Election timeouts are randomized but **deterministic**: each node seeds a small
  internal PRNG (SplitMix64) from its node id, so a multi-node cluster elects a
  stable leader without any real randomness or wall-clock dependence.
- A leader sends `AppendEntries` to replicate entries and, with no entries, as a
  heartbeat on its heartbeat interval. On becoming leader, a node appends a no-op
  to anchor the new term so prior-term entries can commit.
- Followers run the log-consistency check on the entry preceding new entries,
  truncate any conflicting suffix, append the new entries, and advance their
  commit index up to the leader's.
- A leader advances its commit index to the highest index replicated on a
  majority whose entry is from the current term (the Raft commit rule).
- A single voter is its own majority, so a single-node cluster commits its own
  entries immediately.

Newly committed entries are drained by the caller (`take_committed`), which
advances the applied index; the caller applies those entries to the state
machine.

## The deterministic test clock and simulation harness

Because the node is driven by a logical clock rather than wall-clock time, its
behavior is fully reproducible and never timing-flaky. A deterministic in-process
simulation harness wires several nodes together over an in-memory message bus —
no sockets, no real timing — and drives them tick by tick, delivering messages to
quiescence. It supports partitioning and healing nodes. This harness is the
substrate for the multi-node consensus tests and the replication crate's
end-to-end apply tests. See [TESTING.md](TESTING.md).

## Cross-process peer transport (v0.5.0)

v0.5.0 carries the same Raft messages between **separate server processes** over
a dedicated cluster socket, so the deterministic state machine above now drives a
real cross-process cluster:

- **Framing.** Each frame is magic-tagged (`APR1`), protocol-version-tagged
  (v1), length-delimited, and CRC32-checksummed, with a 16 MiB payload-size
  limit. A frame that fails any of these checks is rejected rather than read.
- **Handshake.** A connection opens with a `PeerHello` that verifies the protocol
  version, the cluster id, the peer's node id (against the static membership),
  and a shared authentication token. A wrong-cluster, unknown-node,
  duplicate-node, or bad-token peer is rejected with a structured `PeerError`.
- **Replication over the wire.** Real cross-process leader election,
  AppendEntries replication, majority commit, follower apply, and follower
  catch-up after restart all run over this transport. The leader write path
  blocks until a majority commits; a minority cannot commit. `commit_ts =
  commit_ts_base + raft_log_index`, unchanged from v0.4.x.
- **Connection management.** Reconnect uses bounded backoff (50 ms .. 2 s);
  shutdown is graceful.
- **Peer snapshot install (v0.6.0).** A follower that has fallen behind the
  leader's compacted prefix is brought current by a **bounded, single-message**
  snapshot install over the transport (validated for cluster id, format, digest,
  boundary, storage format, and size), then resumes AppendEntries. This is a
  preview transfer, not chunked streaming. See [REPLICATION.md](REPLICATION.md).
- **Snapshot-needed and follower-lag diagnostics (v0.6.1).** v0.6.1 adds
  per-peer snapshot-needed and lag diagnostics plus metrics over the **unchanged
  v0.6.0 install path**; the transfer itself is not changed. See
  [REPLICATION.md](REPLICATION.md) and [OBSERVABILITY.md](OBSERVABILITY.md).
- **Repeated chaos and recovery validation (v0.6.2).** v0.6.2 adds repeated
  leader restart / re-election cycles, deterministic network-interruption
  (partition/heal) simulations via an in-process transport drop control, and a
  cumulative `leader_changes` recovery signal — all exercising the **same**
  election, log-repair, and commit-advancement code paths under repeated failure.
  Because this Raft does not implement pre-vote, an isolated *running* node's term
  can advance while it is partitioned; on heal the cluster reconverges (a brief
  re-election may occur), which is why the recovery tests assert eventual
  convergence rather than a fixed election outcome. See [TESTING.md](TESTING.md).

The transport is gated behind the two `[cluster]` opt-ins (`enabled = true` and
`experimental_multi_node = true`) and fails closed on a non-loopback address
unless `allow_experimental_public_cluster = true` (which then requires peer TLS
and a token). See [CLUSTERING.md](CLUSTERING.md) and [SECURITY.md](SECURITY.md).

## What this release does not do

The consensus algorithm here is implemented and tested for leader election, log
replication, log repair, and commit advancement — in single-node mode, through
the in-memory simulation, and (in the v0.5.0 preview) across real server
processes over the peer transport. It deliberately does **not** include:

- **Membership changes / joint consensus.** The voter set is fixed; there is no
  `join`, `leave`, or `step-down`. Membership is static.
- **Chunked / streaming snapshot install.** v0.6.0 ships a **bounded,
  single-message** peer snapshot install (see [REPLICATION.md](REPLICATION.md)),
  not chunked streaming of arbitrarily large snapshots.
- **Production-grade peer networking or automatic failover.** The cross-process
  transport is an experimental preview — **not production HA**. Leader kill and
  re-election are a fail-stop recovery preview, not production automatic failover.
  See [CLUSTERING.md](CLUSTERING.md).
