# AuraDB v0.3.1 release notes

**MVCC stabilization, upgrade confidence, and operational guardrails.**

AuraDB v0.3.1 is a stabilization release for the MVCC and query-planner behavior
introduced in v0.3.0. It hardens the transaction lifecycle, adds transaction
timeouts and an abandoned-transaction reaper, strengthens version garbage
collection, and surfaces MVCC pressure through metrics, `status`, and `doctor`
warnings. It preserves all v0.3.0 behavior and remains compatible with Aura
Connector 0.3.x — no connector release is required.

> AuraDB v0.3.1 implements single-node snapshot isolation with optimistic write
> conflict detection. It is **not** serializable isolation. This release adds no
> clustering, replication, Raft, sharding, or distributed transactions.

## Why this release

The v0.3.0 known limitation was that a long-lived or abandoned transaction keeps
the versions visible at its snapshot alive until it ends — and a transaction
handle that was dropped or whose connection vanished could pin those versions
indefinitely, stalling garbage collection with no visibility. v0.3.1 keeps that
behavior correct but adds the operational guardrails to detect, surface, and
recover from it.

## Highlights

### Transaction lifecycle hardening

- An **active transaction registry** tracks every open transaction's id, read
  timestamp, start time, last-activity time, owning connection, and state. GC
  computes its reclamation horizon from this registry, never from stale state.
- **Transaction timeout.** An idle transaction older than
  `[mvcc] transaction_timeout_secs` (default 300s) is reaped: it is marked aborted,
  its snapshot is released so GC can progress, and any further operation on it is
  rejected with a structured `transaction_timeout` error.
- **Abandoned-transaction reaper.** A background task runs every
  `[mvcc] abandoned_transaction_reaper_secs` (default 30s). A transaction handle
  dropped without commit or rollback — which Rust ownership cannot clean up in
  `Drop`, because releasing a snapshot is an engine operation — is reaped here
  instead of pinning versions forever.
- **Connection-scoped cleanup.** When a connection closes, the server rolls back
  every transaction it owns, releasing their snapshots immediately.

### MVCC garbage collection

- GC correctness is validated against active snapshots: it preserves every
  version a live transaction can observe, reclaims superseded versions once no
  reader can see them, keeps the latest committed version of each live record,
  and keeps a tombstone until no old snapshot can see the pre-delete version. GC
  is idempotent and runs after restart and after backup/restore.
- The GC report now includes `bytes_reclaimed`.
- New CLI: `auradb gc --dry-run` reports what would be reclaimed without modifying
  data, and `auradb gc --json` emits a machine-readable report.

### Observability and operational warnings

- New MVCC metrics: `auradb_mvcc_active_transactions`,
  `auradb_mvcc_oldest_snapshot_age_seconds`, `auradb_mvcc_retained_versions`,
  `auradb_mvcc_gc_runs_total`, `auradb_mvcc_gc_reclaimed_versions_total`,
  `auradb_mvcc_gc_reclaimed_bytes_total`, `auradb_mvcc_transaction_timeouts_total`,
  and `auradb_mvcc_conflicts_total`.
- `auradb status` (and the health report) now include an `mvcc` section: active
  transactions, timed-out transactions, oldest snapshot age, retained versions,
  cumulative timeouts, the configured timeout, and whether GC is enabled.
- `auradb doctor` warns on long-lived snapshots, version pressure, disabled GC,
  disabled transaction timeouts, and stale planner statistics. Secrets stay
  redacted.

### EXPLAIN ANALYZE

`EXPLAIN ANALYZE` gains the planner-statistics version, the estimated row count
alongside the measured actuals, a human-readable selected-index reason, the MVCC
snapshot timestamp, and a stale-statistics warning. All are additive JSON fields,
so Aura Connector 0.3.x stays compatible.

### Benchmark regression guardrail

`auradb bench compare --baseline … --current …` compares two benchmark reports,
reports the per-benchmark percent change, and marks large regressions as
warnings. Pass `--fail-threshold-percent` to make CI fail intentionally;
otherwise it never fails normal CI. Benchmarks are hardware- and load-sensitive —
only compare reports produced on the same quiescent machine.

## Configuration

```toml
[mvcc]
gc_enabled = true
gc_interval_secs = 300
min_retained_versions = 1
transaction_timeout_secs = 300
abandoned_transaction_reaper_secs = 30
```

Set `transaction_timeout_secs = 0` to disable timeouts (not recommended:
abandoned transactions then pin versions indefinitely).

## Compatibility

- **Aura Connector 0.3.x:** fully compatible, no release required. The only wire
  additions are additive JSON fields (the health `mvcc` section and extra
  `EXPLAIN ANALYZE` diagnostics), which older clients ignore. Validated locally
  with the published `aura-connector` 0.3.0 against a v0.3.1 server with auth and
  TLS enabled: connector smoke 11/11 and wire conformance 17/17, with no token,
  hash, or private key in the logs.
- **Storage format:** unchanged at v2. A v0.1.0/v0.2.x directory still migrates to
  v2 transparently on first open; a v0.3.0 directory opens directly. An unknown
  future format is rejected. See [UPGRADING.md](UPGRADING.md).
- A new `transaction_timeout` error code is additive; connectors that do not model
  it fall back to a generic server error.

## Upgrading

Upgrading is a drop-in binary replacement. The new `[mvcc]` timeout settings
default safely. See [UPGRADING.md](UPGRADING.md) and [OPERATIONS.md](OPERATIONS.md).

## Known limitations

- Single-node only: no clustering, replication, Raft, sharding, or distributed
  transactions. (Replication and Raft are future work; this release only prepares
  the codebase for them.)
- Snapshot isolation, not serializable isolation.
- A long-lived transaction still pins the versions visible at its snapshot for as
  long as it is active; the timeout and reaper bound how long an *idle or
  abandoned* transaction can do so, and `doctor`/metrics make active long-lived
  transactions visible.
- Vector search is exact (no ANN/HNSW); full-text is the v0.2.x scorer (no
  BM25/hybrid fusion).
