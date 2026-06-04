# Storage Engine

The storage engine (`auradb-storage`) is a persistent, recoverable, append-only
record store.

## Directory layout

```
<data_dir>/
  MANIFEST           JSON: format version, segment list, next ids
  catalog.json       JSON: collection schemas + schema version
  0000000001.seg     append-only segment files (zero-padded id)
  indexes/           persisted index snapshots
    INDEX_MANIFEST.json
    <collection>.idx framed, CRC32-checked index snapshot files
```

## Segment format

A segment is a sequence of **batch frames**. Each frame is:

```
[payload length: u64 BE][payload CRC32: u32 BE][payload bytes]
```

The payload is the JSON encoding of a `Batch { txn_id, ops }`, where each op is
`Put { record }` or `Delete { collection, id }`.

Because a batch is a single length-prefixed, checksummed unit, it is **atomic**:
either the whole batch is durably present or it is not.

## Writes and durability

`commit_batch` appends the encoded batch, flushes, and (by default) `fsync`s the
active segment before returning. `sync_on_commit` can be disabled for bulk
import or benchmarks, trading durability for throughput - this is documented and
opt-in, never silent.

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

`compact` rewrites all live records into a single fresh segment, atomically
swaps the manifest to point only at it, and removes the old segments. Live data
is preserved; dead versions and tombstones are discarded. The operation is
crash-safe: the new segment and manifest are written and fsynced before old
segments are removed.

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

## Identity boundary

Records are addressed by stable logical `RecordId`. Physical offsets are **never**
exposed as durable identity, so compaction and recovery can relocate data freely
Physical-offset link optimization is future work; see [ROADMAP](ROADMAP.md).

## Tests

`write_and_read`, `delete_removes`, `restart_persistence`, `scan_and_count`,
`schema_catalog_persists`, `checksum_corruption_detected`,
`compaction_preserves_live_data`, `drop_schema_removes_records`, plus
format-level torn-tail and corruption tests. Deterministic seeded recovery tests
(`crates/auradb-storage/tests/recovery.rs`, `crates/auradb/tests/recovery.rs`)
cover randomized operation sequences against a reference model with and without a
checkpoint, trailing-segment truncation, mid-batch byte-flip detection, catalog
corruption (fail closed), and corrupt/missing index file and manifest repair.
