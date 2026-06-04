# Testing

All tests are deterministic and use `tempfile` for isolated database directories.

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-features
```

## Coverage by area

- **Protocol** (`auradb-protocol`): frame roundtrip, unknown magic, bad version,
  bad header/payload checksum, oversized payload, unknown opcode, truncated
  frame, error frame encoding, cursor messages.
- **Storage** (`auradb-storage`): write/read, delete, restart persistence,
  checksum corruption detection, manifest persistence, schema catalog
  persistence, scan, compaction preserves data.
- **Transactions** (`auradb-txn` + `auradb`): commit persists, rollback discards,
  read-your-writes, multi-record atomicity, restart after commit, restart after
  rollback, write-write conflict.
- **Index** (`auradb-index`): primary lookup, unique violation, secondary filter,
  rebuild after restart, delete removes entry, update moves entry, vector exact
  nearest.
- **Query** (`auradb-query` + `auradb`): find, filter, comparisons, contains,
  AND/OR, order, limit, offset, insert, bulk insert, update, delete, upsert,
  count, exists, select projection, include relationships, document field
  access, vector nearest, explain.
- **Schema**: registration, persistence, validation on writes, vector dimension
  validation, unique, migration impact estimate.
- **Cursors** (`auradb-server`): create, fetch page, close, timeout, early close,
  bounded memory.
- **Server / integration** (`tests/integration`): end-to-end client → server for
  ping, health, schema, CRUD, stream, vector, explain, migration estimate.
- **Recovery** (`tests/recovery`): kill-and-reopen persistence and torn-tail
  truncation.
- **Seeded recovery/fuzz** (`crates/auradb-storage/tests/recovery.rs`,
  `crates/auradb/tests/recovery.rs`): deterministic, fixed-seed randomized tests
  (never flaky) covering random insert/update/delete sequences verified against a
  reference model after restart (with and without a checkpoint), trailing-segment
  truncation recovery, mid-batch byte-flip corruption detection, catalog
  corruption detection (fail closed), corrupt/missing index file repair, and
  corrupt index manifest repair.
- **Conformance** (`auradb-conformance`, `tests/conformance`): the full Aura
  Connector scenario list run over the wire protocol.

## Honesty check

Production code must not ship incomplete-code markers or unimplemented features.
A repository scan greps the source tree for incomplete-code macros and
unfinished-work vocabulary to ensure no unfinished behavior is presented as
working. Unsupported operations must instead return a structured
`Error::Unsupported`.
