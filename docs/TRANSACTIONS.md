# Transactions

`auradb-txn` plus the engine implement single-node transactions.

## Model

A transaction stages its mutations in memory and records the **version it
observed** for every record it reads or writes (`None` = observed absent).
Staged writes are invisible to other transactions and provide read-your-writes
for point reads within the transaction.

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

**Snapshot reads with optimistic write/read conflict detection on commit.** This
is honest about what it is: it detects concurrent modification of any record the
transaction touched. It is **not** serializable MVCC, and AuraDB does not claim
to be. Distributed transactions are not implemented.

## Auto-commit

Mutations sent without a transaction id are applied immediately as a single
atomic batch (no conflict window).

## Durability and recovery

A committed transaction is durable once `commit` returns (the batch is fsynced
when `sync_on_commit` is enabled). On restart, only durably committed batches
are replayed; uncommitted staged writes never reached disk.

## Tests

`transaction_commit_persists_across_restart`, `transaction_rollback_discards`,
`transaction_conflict_detected`, read-your-writes via `txn_get`, multi-record
atomicity, and over-the-wire transaction scenarios in the conformance suite.
