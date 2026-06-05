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
- **MVCC and GC** (`crates/auradb/tests/mvcc.rs`): snapshot isolation (a
  transaction does not see a later commit), read-your-writes over the snapshot
  (staged insert/update visible, staged delete hidden), non-transactional reads
  see the latest committed state, write-write / update-delete / delete-update
  conflict rejection, monotonic commit timestamps, cursors and relationship /
  vector / document-path / full-text reads holding their snapshot, and version GC
  reclaiming old versions while preserving any version a live snapshot can see.
  Storage-level MVCC and GC unit tests live in `auradb-storage`.
- **Planner and statistics** (`crates/auradb/tests/planner.rs`): cost-based
  access-path selection (the planner uses an index for a selective equality),
  `EXPLAIN ANALYZE` shape (plan tree plus execution metrics), and planner
  statistics persistence (`planner_stats.json` written, reloaded on open, and
  tolerant of a missing or corrupt file). Planner and stats unit tests live in
  `auradb-query`.
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
- **MVCC upgrade** (`crates/auradb/tests/upgrade_v0_2_x.rs`): open committed
  v0.2.0 and v0.2.1 data directories (storage format v1, written by the respective
  release binaries) with the v0.3.0 engine; verify the v1-to-v2 migration runs on
  first open, existing records become the first committed version on their chains,
  planner statistics initialize, lookups work against the migrated store, and
  `auradb check` passes.
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
  the v0.3.1 release these were validated locally with `aura-connector` 0.3.0
  (from PyPI) in plaintext, auth, and TLS-plus-auth modes — the connector smoke
  passing 11/11 and the standard-library wire conformance 17/17 over TLS plus
  auth — with no token, token hash, or private key in the server logs. For the
  **v0.4.1** release the same published `aura-connector` 0.3.0 was re-run against
  a freshly built server in both non-cluster and single-node cluster modes — the
  connector smoke passing 12/12 and the standard-library wire conformance 18/18
  in each mode. See [CONFORMANCE.md](CONFORMANCE.md).
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

## MVCC stabilization suites (v0.3.1)

- `crates/auradb/tests/transaction_lifecycle.rs` — the active transaction
  registry, transaction timeout, abandoned-transaction reaper, GC-progresses-
  after-timeout, status, and metrics. A controllable `WallClock` drives timeouts
  deterministically; there are no sleep-based tests.
- `crates/auradb/tests/gc_validation.rs` — GC idempotence, snapshot-reader
  retention, removal after release, tombstone visibility, after-restart, index and
  planner-stats consistency, and the reclaimed versions/bytes report.
- `crates/auradb/tests/upgrade_to_v0_3_1.rs` — opens genuine v0.1.0/v0.2.0/v0.2.1/
  v0.3.0 release fixtures with the v0.3.1 engine and runs the full upgrade
  checklist, including rejection of an unknown future format.
- `crates/auradb/tests/planner_regression.rs` and
  `crates/auradb/tests/explain_analyze_fields.rs` — planner access-path selection,
  correctness under stale stats, and the `EXPLAIN ANALYZE` shape.
- `crates/auradb-cli/tests/backup_restore.rs` — backup/restore combined with GC
  (latest-state semantics, no resurrection of reclaimed versions).

## Cluster and replication suites (v0.4.0)

These suites cover the Raft and replication groundwork. The consensus tests are
deterministic — they are driven by a logical clock and an in-memory message bus,
never wall-clock timing — so they are reproducible and never flaky.

- **Cluster metadata** (`auradb-cluster`) — node/cluster id generation, hex
  display and round-trip, durable identity init/load/reopen, idempotent init,
  pinned-id mismatch rejection, rejection of an unknown future `format_version`,
  malformed/partial identity rejection, and `[cluster]` config validation.
- **Raft log** (`auradb-raft`) — append with the no-gap and no-term-regression
  invariants, suffix truncation, the in-memory and file backends, durable
  persistence across reopen, hard-state persistence, checksum-corruption detection
  (fail closed), and torn-trailing-frame truncation on open.
- **Raft consensus / state machine** (`auradb-raft`) — leader election, log
  replication, log repair, and commit advancement, run multi-node through the
  deterministic in-process simulation harness (with partition/heal), plus
  single-node election.
- **Replicated apply** (`auradb-replication`) — the replicated command model and
  versioned encoding (rejecting a newer envelope), the idempotent apply path
  (commit timestamp equals log index), and follower-write `not_leader` rejection.
- **Snapshot** (`auradb-replication`) — snapshot create/restore round-trip,
  version and digest verification, and rejection of an unknown future snapshot
  format.
- **Single-node cluster mode** — a durable single-node cluster orders writes
  through the Raft log, elects itself leader, and replays committed-but-unapplied
  entries on restart.

See [CLUSTERING.md](CLUSTERING.md), [RAFT.md](RAFT.md), and
[REPLICATION.md](REPLICATION.md).

## Multi-node preview suites (v0.5.0)

> **AuraDB v0.5.0 introduces a controlled, experimental multi-node server
> preview. Single-node mode remains the recommended production mode.**

These integration suites stand up **multiple real server processes over real TCP
sockets** (the loopback three-node configuration) and exercise the cross-process
cluster. They use **readiness/bounded polling** (for example
`auradb cluster wait-ready` / `wait-leader`) rather than fixed sleeps, so they are
not timing-flaky.

- **Peer transport** — the frame codec (magic `APR1`, protocol version v1,
  length-delimited, CRC32, 16 MiB payload cap) round-trips and rejects bad
  frames; the `PeerHello` handshake accepts a valid peer and rejects
  wrong-cluster, unknown-node, duplicate-node, and bad-token peers with a
  structured `PeerError`; a snapshot-install request is answered with the
  structured *unsupported* response; reconnect uses bounded backoff and shutdown
  is graceful.
- **Cross-process replication** — real leader election across processes,
  AppendEntries replication, majority commit (a minority cannot commit), follower
  apply, and follower catch-up after restart.
- **Leader/follower client behavior** — writes go to the leader; a follower
  rejects writes with a structured `not_leader` error and a leader hint while the
  connection stays healthy; followers reject reads; the `cluster` status section
  reports `preview_multi_node`, `quorum_available`, and per-peer state.
- **CLI cluster commands** — the live `auradb cluster leader`, `wait-leader`, and
  `wait-ready` commands against a running server (text and `--json`), including
  the readiness/leader-detection polling and detecting when no leader is known
  yet.

## Multi-node preview hardening suites (v0.5.1)

> **AuraDB v0.5.1 hardens the controlled multi-node preview. Single-node mode
> remains the recommended production mode.**

- **Leader restart and re-election** (`crates/auradb-replication/tests/multi_node.rs`):
  stopping the leader lets the surviving majority elect a new leader and keep
  accepting writes; the restarted old leader rejoins as a follower, catches up,
  and all nodes converge on an identical record set. This is preview
  leader-restart behavior, not production failover.
- **Follower catch-up under larger logs** (same file): a follower that misses a
  long run of committed entries (1,000+ entries, batched commits, and across the
  commit-base/snapshot boundary) replays its durable log and is brought current,
  with matching applied indices and record counts. A snapshot install that the
  preview does not implement is answered with a structured *unsupported*
  response, never silent corruption or a hang.
- **`not_leader` ergonomics** (`crates/auradb-server/tests/not_leader.rs`,
  `cluster_preview.rs`): the error carries the leader hint and the leader's client
  address (when a peer declared one), is marked `retryable`, and the same client
  connection stays usable afterward.
- **Cluster diagnostics** (`cluster_preview.rs`, `multi_node.rs`): health and
  `cluster status --addr` report the leader's client address, per-peer
  reachability, and connection attempts; an unreachable peer and a lost quorum are
  visible.
- **Peer TLS validation** (`crates/auradb-server/tests/peer_tls.rs`): a real
  mutual-TLS peer handshake succeeds for valid material and is rejected for a
  wrong CA or a non-matching SAN; a certificate rotated under the same CA is
  accepted.

See [CLUSTERING.md](CLUSTERING.md), [RAFT.md](RAFT.md),
[REPLICATION.md](REPLICATION.md), and [CONFORMANCE.md](CONFORMANCE.md).

## Raft durability and cluster-mode hardening suites (v0.4.1)

These suites harden the v0.4.0 groundwork. All are deterministic — multi-node
behavior is driven by the in-process simulation with a logical clock, so there are
no flaky sleeps.

- **Raft log compaction** (`auradb-raft`) — the compactable-prefix calculation
  refuses to discard unapplied or uncommitted entries, preserves the last included
  index/term, returns a structured `Compacted` error for reads before the prefix,
  understands the boundary in the AppendEntries check, persists across restart, and
  fails closed on corrupt or disagreeing compaction metadata.
- **Snapshot restore edge cases** (`auradb-replication`) — atomic restore that
  rejects future formats, cluster-id mismatch, corrupt payloads, and a non-empty
  target without `--force`, and preserves existing data on a validation failure;
  plus index/stats rebuild and manifest inspection.
- **Apply idempotency under restart** (`auradb-replication`) — committed entries
  apply once across restarts; commit-before-apply, partial apply, and
  apply-before-watermark sequences recover without duplicates; uncommitted entries
  are not applied.
- **Cluster metadata corruption** (`auradb-cluster`) — missing, malformed,
  future-format, partial, and id-mismatch identity is rejected (fail closed), and
  peer configuration is validated (duplicate / self / invalid peers).
- **Deterministic multi-node partitions** (`auradb-raft`) — minority cannot
  commit, majority elects a leader, the old leader steps down on rejoin, committed
  entries survive a leader change, and an uncommitted old-leader entry is repaired
  away.
- **`not_leader` over the wire** (`auradb-server`) — a non-leader write returns a
  structured `not_leader` error with a hint, the connection stays healthy, and the
  response is prompt and terminal (no internal retry loop).
- **v0.4.1 upgrade** (`auradb`) — pre-0.4.x fixtures and the v0.4.0 cluster layout
  open unchanged; compaction metadata initializes safely; v0.4.0 snapshot manifests
  still decode; future formats are rejected.
- `cmd_bench_compare` unit tests cover the benchmark regression comparison logic.
