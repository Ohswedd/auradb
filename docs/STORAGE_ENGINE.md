# Storage Engine

The storage engine (`auradb-storage`) is a persistent, recoverable, append-only
record store.

## Directory layout

```
<data_dir>/
  MANIFEST           JSON: format version, last_commit_ts, segment list, next ids
  catalog.json       JSON: collection schemas + schema version
  planner_stats.json JSON: persisted planner statistics (advisory)
  0000000001.seg     append-only segment files (zero-padded id)
  indexes/           persisted index snapshots
    INDEX_MANIFEST.json
    <collection>.idx framed, CRC32-checked index snapshot files
  cluster/           cluster identity + Raft log (only when cluster mode is used)
    node.json        this node's stable id
    cluster.json     the cluster this node belongs to
    raft-log.bin     framed, CRC32-checked Raft log entries
    raft-state.json  Raft hard state (term, vote, commit index)
```

The `cluster/` directory exists only when cluster mode is enabled (or after
`auradb cluster init`); it is a **separate durable log** and does not change the
regular storage format. See [RAFT.md](RAFT.md) and [CLUSTERING.md](CLUSTERING.md).

## Storage format and version chains

The on-disk format is **v2** (`FORMAT_VERSION = 2`). **Storage format v2 is frozen
for v1:** it is the stable v1 single-node storage format, and AuraDB v1.x preserves
storage format v2 compatibility unless a safety, corruption, or security issue
requires a documented migration. Each record id maps to an ordered **version
chain** — a list of `Version { commit_ts, value }` where `value` is the record and
`value = None` is a **tombstone** (a committed delete).
Versions are ordered by their commit timestamp. Every operation in the log
carries a `commit_ts`, and the manifest tracks `last_commit_ts` so the commit
clock never regresses across restarts.

A v1 directory (AuraDB ≤ 0.2.x) is migrated to v2 transparently the first time
v0.3.0 opens it: each existing record becomes the first committed version on its
chain. An unknown future format is still rejected rather than opened. See
[UPGRADING.md](UPGRADING.md).

The **storage format is unchanged in v0.4.0**. When cluster mode is enabled, the
MVCC commit timestamp equals the Raft log index — the commit clock is driven by
the ordered Raft log rather than incremented inline — so commit timestamps stay
monotonic and deterministic across replicas. When cluster mode is disabled (the
default), commit timestamps are assigned exactly as in v0.3.1. See
[REPLICATION.md](REPLICATION.md).

## Segment format

A segment is a sequence of **batch frames**. Each frame is:

```
[payload length: u64 BE][payload CRC32: u32 BE][payload bytes]
```

The payload is the JSON encoding of a `Batch { txn_id, commit_ts, ops }`, where
each op is `Put { record }` or `Delete { collection, id }`. The batch's
`commit_ts` stamps every version it appends.

Because a batch is a single length-prefixed, checksummed unit, it is **atomic**:
either the whole batch is durably present or it is not. All operations in a batch
share one commit timestamp, so a transaction's writes become visible together.

## Writes and durability

`commit_batch` appends the encoded batch, flushes, and (by default) `fsync`s the
active segment before returning. `sync_on_commit` can be disabled for bulk
import or benchmarks, trading durability for throughput - this is documented and
opt-in, never silent.

## Reads: latest vs as-of

The storage layer serves two kinds of read over the version chains:

- **Latest committed** — `get` and `scan` return the newest non-tombstone version
  on each chain. This is what non-transactional reads use, unchanged from v0.2.1.
- **Snapshot (as-of)** — `get_as_of(read_ts)` and `scan_as_of(read_ts)` return the
  newest version whose `commit_ts <= read_ts`, skipping records whose visible
  version is a tombstone. Transactions read this way, pinning `read_ts` at `begin`.

`commit_watermark()` reports the timestamp a new transaction would pin at `begin`,
and `latest_commit_ts()` reports the most recent commit. See
[TRANSACTIONS.md](TRANSACTIONS.md).

## Recovery

On open, every segment is replayed in order:

- A **torn trailing batch** (declared length runs past end-of-file) is detected,
  the file is truncated to the last valid byte, and recovery continues. Only the
  active (last) segment may be torn; a torn batch in a sealed segment is treated
  as corruption.
- A **checksum mismatch** on a fully present batch is reported as
  `Error::Corruption` - the engine fails closed rather than dropping committed
  data.

The highest transaction id seen during replay seeds the logical clock so ids
never regress across restarts.

## Compaction

`compact` rewrites the store into a single fresh segment, atomically swaps the
manifest to point only at it, and removes the old segments. Compaction
**preserves all versions** on each chain (it does not reclaim history); use
version GC to reclaim old versions. The operation is crash-safe: the new segment
and manifest are written and fsynced before old segments are removed. Compaction
also refreshes persisted planner statistics.

## Version garbage collection

Because each record keeps a version chain, old versions are reclaimed by
`Storage::gc(cutoff, min_retained)`:

- It removes versions older than `cutoff` that no reader at or after `cutoff` can
  observe, always keeping the latest version and at least `min_retained` versions
  per chain.
- A record whose latest version is a tombstone older than `cutoff` is dropped
  entirely.

The engine drives GC with a horizon derived from the oldest active transaction
snapshot in the active transaction registry (or the commit watermark when no
transaction is active), so a version a live transaction can still see is never
reclaimed. A timed-out transaction is excluded from the horizon, so reaping an
abandoned transaction lets GC reclaim the versions it had pinned. GC runs via
`auradb gc` or optional background GC configured under `[mvcc]`. The `GcReport`
includes versions reclaimed, records removed, versions retained, and
`bytes_reclaimed` (the segment-size delta across the rewrite); `Storage::gc_preview`
backs `auradb gc --dry-run`, computing the counts without modifying data. See
[CONFIGURATION.md](CONFIGURATION.md), [TRANSACTIONS.md](TRANSACTIONS.md), and
[OPERATIONS.md](OPERATIONS.md).

### Backup semantics

The logical dump (`auradb dump`/`restore`) exports the **latest committed visible
state**, not the full version history. A restore therefore starts each record with
a single fresh version and never resurrects a version GC reclaimed. This is a
logical latest-state backup; there is no full-history dump.

## Persisted index snapshots

Indexes are snapshotted to disk so they are not always rebuilt from the record
log on open. Each per-collection `.idx` file carries a framed header: a 4-byte
magic `AIDX`, a `u32` format version, a `u32` payload length, a `u32` CRC32 of the
payload, then a JSON payload. An `INDEX_MANIFEST.json` tracks the set.

- **Checkpoints.** Snapshots are written at `compact`, graceful server shutdown,
  and `auradb index rebuild`.
- **Staleness detection.** On open, a snapshot is loaded only when its content
  fingerprint (an FNV-1a hash over each record's id and version) matches the
  current storage state, its index field shape matches the schema, and its CRC is
  valid.
- **Safe rebuild.** If any of those checks fail (including a crash between
  checkpoints, a corrupt or missing `.idx` file, or a corrupt manifest), the
  engine rebuilds the affected index from the durable record log and records that
  it rebuilt. The record log remains the source of truth, so queries never return
  incorrect results from a stale or damaged snapshot.

The persisted kinds are primary key, unique, secondary, document-path,
full-text, and exact vector. See [INDEXING.md](INDEXING.md).

## Structured consistency report

`auradb check --json` (v0.8.0) surfaces a structured consistency report across the
storage, catalog, indexes, planner statistics, Raft log, and snapshot boundaries
(top-level `ok`, `storage`, `catalog`, `indexes`, `planner_stats`, `raft`,
`snapshots`, `warnings`, `errors`). It detects segment-checksum, manifest, catalog,
index-manifest, planner-stats, raft-log, and snapshot-boundary corruption — a
recoverable index-manifest mismatch is rebuilt and reported as a warning, advisory
planner-stats problems are warnings, and an unknown future storage format is
rejected — and the command exits non-zero if any check fails, so it can be
scheduled and alerted on. The report never prints secrets. See [CLI.md](CLI.md) and
[OBSERVABILITY.md](OBSERVABILITY.md).

## Identity boundary

Records are addressed by stable logical `RecordId`. Physical offsets are **never**
exposed as durable identity, so compaction and recovery can relocate data freely
Physical-offset link optimization is future work; see [ROADMAP](ROADMAP.md).

## Tests

`write_and_read`, `delete_removes`, `restart_persistence`, `scan_and_count`,
`schema_catalog_persists`, `checksum_corruption_detected`,
`compaction_preserves_live_data`, `drop_schema_removes_records`, plus
format-level torn-tail and corruption tests. MVCC storage unit tests cover
version-chain reads (`get`/`scan` latest versus `get_as_of`/`scan_as_of`
snapshot), tombstone visibility, the commit watermark, `gc` reclaiming old
versions while keeping the latest and at least `min_retained`, compaction
preserving all versions, and the v1-to-v2 migration on open. Deterministic seeded
recovery tests
(`crates/auradb-storage/tests/recovery.rs`, `crates/auradb/tests/recovery.rs`)
cover randomized operation sequences against a reference model with and without a
checkpoint, trailing-segment truncation, mid-batch byte-flip detection, catalog
corruption (fail closed), and corrupt/missing index file and manifest repair.
