# Raft Consensus Core

AuraDB v0.4.0 includes a minimal, deterministic Raft log and consensus core that
underpins cluster mode. This document describes the log model, the durable
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

## What this release does not do

The consensus algorithm here is implemented and tested for leader election, log
replication, log repair, and commit advancement, in single-node mode (in the
server) and multi-node mode (through the in-memory simulation). It deliberately
does **not** include:

- **Membership changes / joint consensus.** The voter set is fixed; there is no
  `join`, `leave`, or `step-down`.
- **Log snapshots / compaction** beyond the snapshot boundary defined in the
  replication layer (see [REPLICATION.md](REPLICATION.md)).
- **Real network transport.** Multi-node consensus runs only through the
  deterministic in-memory harness; cross-process transport is not part of this
  release, and multi-node server deployment is rejected at startup. See
  [CLUSTERING.md](CLUSTERING.md).
