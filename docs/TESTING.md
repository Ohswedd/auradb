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
- **Backup / restore** (`crates/auradb-cli/tests/backup_restore.rs`): dump and
  restore a database containing scalar, document, vector, relationship,
  full-text, and document-path data, then verify records, schema, every index
  kind, search, relationship include, count, exists, and `auradb check` on the
  restored directory.
- **Upgrade** (`crates/auradb/tests/upgrade_v0_1_0.rs`): open a committed v0.1.0
  data directory (written by the v0.1.0 binary) with the current engine; verify
  the catalog and records load, indexes rebuild from storage, rebuilt indexes
  serve lookups, `auradb check` passes, a post-upgrade backup round-trips, and an
  unknown future storage format is rejected rather than silently opened.
- **Chaos restart** (`crates/auradb/tests/chaos_restart.rs`): a deterministic,
  seeded stream of writes, updates, deletes, and transactions with the engine
  dropped and reopened from disk at fixed intervals, comparing the recovered
  state (records and every index kind) against a reference model after each
  restart, plus a dump/restore check. A heavier stress run is available behind
  `--ignored`.
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
  Connector scenario list run over the wire protocol. In addition to the Rust and
  standard-library Python harnesses, the published Aura Connector drives the
  server through `run_connector_smoke.py` and `run_connector_conformance.py`. For
  the v0.2.1 release these were validated locally with `aura-connector` 0.3.0
  (from PyPI) in plaintext, auth, and TLS-plus-auth modes, with no secret in the
  server logs. See [CONFORMANCE.md](CONFORMANCE.md).
- **Secure deployment** (`docker-compose.secure.yml`): the secure Compose example
  was validated at runtime with development certificates and a generated token
  hash. The container reports healthy over TLS with authentication, a plaintext
  client is rejected, the connector smoke passes against it over TLS plus auth,
  and the token, its hash, and the private key never appear in the container
  logs. See [DEPLOYMENT.md](DEPLOYMENT.md).

## Honesty check

Production code must not ship incomplete-code markers or unimplemented features.
A repository scan greps the source tree for incomplete-code macros and
unfinished-work vocabulary to ensure no unfinished behavior is presented as
working. Unsupported operations must instead return a structured
`Error::Unsupported`.
