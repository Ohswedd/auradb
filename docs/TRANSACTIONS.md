# Transactions

`auradb-txn` plus the engine implement single-node transactions with
**snapshot isolation**.

> AuraDB v0.3.0 implements single-node snapshot isolation with optimistic write
> conflict detection. It is not serializable isolation.

## Model

A transaction pins a **read timestamp** (`read_ts`) at `begin`: the MVCC commit
watermark at that instant. Every read inside the transaction sees committed
state **as of `read_ts`** (resolved against storage version chains), overlaid
with the transaction's own staged writes (read-your-writes). The transaction
does not observe writes committed by other transactions after it began.

A transaction stages its mutations in memory; staged writes are invisible to
other transactions and provide read-your-writes within the transaction, for
point reads (`txn_get`) **and** for every query read (find, filter, count,
exists, explain, vector, document-path, full-text, relationship include, and
cursor paging — all evaluated against the snapshot).

## Transaction-scoped reads

A read that carries a transaction id executes against the **transaction view**:
the committed state overlaid with the transaction's own staged writes and
deletes.

- A staged insert/update is visible to the transaction's reads before commit.
- A staged delete is hidden from the transaction's reads.
- These effects are invisible to non-transactional readers until commit.

Index-seeded selection (equality lookup, vector nearest, full-text) is served
from an **overlay index** the engine builds over the transaction view for the
queried collection, so a staged write is never missed and a staged delete is
never surfaced. Relationship hydration resolves links through the same view.

Reads with no transaction id are completely unaffected and take their prior
committed-state path. The server routes a request to the transaction view
whenever the request frame carries a non-zero `txn_id`; cursors opened inside a
transaction are paged through that same transaction.

This favors correctness over performance: the overlay index is rebuilt for each
transactional query. Transactions are expected to be small, and the cost is
bounded by the queried collection's size; non-transactional reads pay nothing.

## Commit

Commit runs under the engine's single write lock:

1. **Write-conflict detection** (first-committer-wins) - for every record the
   transaction wrote, the latest committed version's commit timestamp must not be
   newer than the transaction's `read_ts`. If another transaction committed a
   write to the same record after this one's snapshot was pinned, commit aborts
   with `Error::Conflict`. This covers write-write, update-delete, and
   delete-update conflicts.
2. **Validation** - uniqueness and referential integrity are checked for the
   final staged records.
3. **Versioning** - the batch is stamped with a fresh commit timestamp; each put
   appends a new version to its chain (and gets `previous_version + 1`), each
   delete appends a tombstone version.
4. **Durable commit** - all operations are written as one atomic storage batch
   at a single commit timestamp.
5. **Index update** - secondary/unique/vector indexes are updated to the latest
   committed state.

Rollback simply discards the staged set and releases the snapshot; nothing was
written.

## Isolation level

**Single-node snapshot isolation with optimistic write-conflict detection.** A
transaction reads a consistent snapshot pinned at `begin` (overlaid with its own
writes) and never observes another transaction's later commit. At commit,
first-committer-wins write-conflict detection aborts a transaction whose write
set was modified concurrently. This is **not** serializable isolation (it does
not prevent write-skew anomalies), and AuraDB does not claim more. Distributed
transactions are not implemented.

## Garbage collection

Because each record keeps a version chain, old versions are reclaimed by version
GC (`auradb gc`, or background GC when enabled). GC removes only versions no
active transaction can observe — it preserves every version visible to the
oldest pinned snapshot, always keeps the latest version, and drops records whose
latest version is a tombstone older than that horizon. See
`docs/STORAGE_ENGINE.md`.

An open transaction holds its snapshot — and therefore the versions visible to
it — until it commits or rolls back. The server releases a transaction's
snapshot when the transaction ends **or when its connection closes**: a
disconnect rolls back any open transaction through the engine, so an abandoned
connection never pins a snapshot indefinitely. A long-running transaction that
stays open does hold old versions until it finishes; the number of transactions
currently holding a snapshot is reported by `EngineStats::active_transactions`
and the server's `active_transactions` metric.

## Auto-commit

Mutations sent without a transaction id are applied immediately as a single
atomic batch (no conflict window).

## Durability and recovery

A committed transaction is durable once `commit` returns (the batch is fsynced
when `sync_on_commit` is enabled). On restart, only durably committed batches
are replayed; uncommitted staged writes never reached disk.

## Tests

`transaction_commit_persists_across_restart`, `transaction_rollback_discards`,
`transaction_conflict_detected`, read-your-writes via `txn_get`, and multi-record
atomicity, plus the transaction-scoped read suite (`crates/auradb/tests/transactions.rs`):

- `transactional_find_sees_staged_insert`, `transactional_filter_sees_staged_insert`
- `transactional_count_sees_staged_insert`, `transactional_exists_sees_staged_insert`
- `transactional_read_hides_staged_delete`, `transactional_update_visible_before_commit`
- `transactional_vector_query_sees_staged_vector`,
  `transactional_document_filter_sees_staged_document_update`
- `transactional_full_text_sees_staged_text_update`,
  `transactional_cursor_uses_transaction_view`
- `rollback_removes_transaction_view_changes`,
  `non_transactional_reader_does_not_see_uncommitted_writes`

plus `transactional_read_sees_staged_write_over_the_wire` (server dispatch) and
the over-the-wire transaction scenarios in the conformance suite.

The snapshot-isolation suite (`crates/auradb/tests/mvcc.rs`) covers
`snapshot_does_not_see_later_commit`, `transaction_sees_own_insert_update_and_hides_delete`,
`non_transactional_read_sees_latest`, `write_write_conflict_rejected`,
`update_delete_conflict_rejected`, `delete_update_conflict_rejected`,
`rollback_discards_versions`, `commit_assigns_monotonic_commit_ts`,
`concurrent_readers_keep_snapshot`, `cursor_keeps_snapshot_after_later_commit`,
`relationship_include_uses_snapshot`, `vector_nearest_uses_snapshot`,
`document_path_index_uses_snapshot`, `full_text_uses_snapshot`, and
`gc_reclaims_old_versions_but_keeps_active_snapshot`.

## Transaction lifecycle and timeouts (v0.3.1)

A transaction pins the versions visible at its snapshot until it commits, rolls
back, or is reaped. The engine keeps an **active transaction registry** — id, read
timestamp, start time, last-activity time, owning connection, and state — and GC
computes its reclamation horizon from this registry, never from stale state.

To bound how long an idle or abandoned transaction can pin versions:

- A transaction idle longer than `[mvcc] transaction_timeout_secs` (default 300s)
  is reaped: marked aborted, its snapshot released so GC can progress, and any
  further operation rejected with a structured `transaction_timeout` error.
- The abandoned-transaction reaper runs every
  `[mvcc] abandoned_transaction_reaper_secs` (default 30s). A transaction handle
  dropped without commit or rollback cannot be cleaned up in `Drop` (releasing a
  snapshot is an engine operation), so the reaper handles it.
- On connection close, the server rolls back every transaction the connection
  owned.

A long-lived but *active* transaction still legitimately pins its snapshot; the
timeout bounds idleness, and `auradb status`/`doctor` and the
`auradb_mvcc_*` metrics make active long-lived snapshots visible. The lifecycle is
covered by `crates/auradb/tests/transaction_lifecycle.rs` (registry, timeout,
reaper, GC-progresses-after-timeout, status, metrics — all driven by a
controllable clock so the tests are deterministic). See
[OPERATIONS.md](OPERATIONS.md) and the `[mvcc]` section of
[CONFIGURATION.md](CONFIGURATION.md).
