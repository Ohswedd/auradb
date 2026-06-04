# Storage Engine

The storage engine (`auradb-storage`) is a persistent, recoverable, append-only
record store.

## Directory layout

```
<data_dir>/
  MANIFEST           JSON: format version, segment list, next ids
  catalog.json       JSON: collection schemas + schema version
  0000000001.seg     append-only segment files (zero-padded id)
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

## Identity boundary

Records are addressed by stable logical `RecordId`. Physical offsets are **never**
exposed as durable identity, so compaction and recovery can relocate data freely
Physical-offset link optimization is future work; see [ROADMAP](ROADMAP.md).

## Tests

`write_and_read`, `delete_removes`, `restart_persistence`, `scan_and_count`,
`schema_catalog_persists`, `checksum_corruption_detected`,
`compaction_preserves_live_data`, `drop_schema_removes_records`, plus
format-level torn-tail and corruption tests.
