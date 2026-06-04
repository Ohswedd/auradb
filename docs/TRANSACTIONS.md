# Transactions

`auradb-txn` plus the engine implement single-node transactions.

## Model

A transaction stages its mutations in memory and records the **version it
observed** for every record it reads or writes (`None` = observed absent).
Staged writes are invisible to other transactions and provide read-your-writes
within the transaction, for point reads (`txn_get`) **and** for every query
read (find, filter, count, exists, explain, vector, document-path, full-text,
relationship include, and cursor paging).

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

1. **Conflict detection** - for every observed `(key, version)`, the current
   committed version must still match. Any mismatch aborts with
   `Error::Conflict`.
2. **Validation** - uniqueness and referential integrity are checked for the
   final staged records.
3. **Versioning** - each put gets `previous_version + 1` (or 1 for new records).
4. **Durable commit** - all operations are written as one atomic storage batch.
5. **Index update** - secondary/unique/vector indexes are updated to match.

Rollback simply discards the staged set; nothing was written.

## Isolation level

**Read-your-writes over the committed state, with optimistic write/read conflict
detection on commit.** A transaction's reads see committed data overlaid with
its own staged changes; commit detects concurrent modification of any record the
transaction observed. This is honest about what it is: it is **not** serializable
MVCC, and reads observe other transactions' commits (the view is not pinned to a
begin-time snapshot). AuraDB does not claim more. Distributed transactions are
not implemented.

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
