# Operations

Running AuraDB in production: MVCC version pressure, transaction lifecycle,
garbage collection, backup, and the signals that tell you the store is healthy.
This is single-node operations; AuraDB has no clustering, replication, or
failover.

## MVCC version pressure

AuraDB keeps multiple committed versions of each record so transactions can read
from a consistent snapshot. Versions that no active transaction can observe are
reclaimed by garbage collection. Two things keep versions alive:

1. **Active transactions.** A transaction pins the versions visible at its read
   timestamp until it commits, rolls back, or is reaped.
2. **Retained minimum.** GC always keeps at least `[mvcc] min_retained_versions`
   (and the latest) versions of every live record.

If versions accumulate, look for a long-lived or abandoned transaction holding an
old snapshot, or a disabled GC.

## Transaction lifecycle and timeouts

A transaction that is never committed or rolled back would pin its snapshot
forever. AuraDB bounds this:

- `[mvcc] transaction_timeout_secs` (default 300): a transaction idle for longer
  is reaped — marked aborted, its snapshot released, and further operations on it
  rejected with a structured `transaction_timeout` error.
- `[mvcc] abandoned_transaction_reaper_secs` (default 30): how often the reaper
  runs.
- On connection close, the server rolls back every transaction the connection
  owned.

Set `transaction_timeout_secs = 0` to disable timeouts. This is **not
recommended**: an abandoned transaction then pins versions indefinitely.

## Garbage collection

```bash
auradb gc --data-dir /data            # reclaim now
auradb gc --data-dir /data --dry-run  # report what would be reclaimed, change nothing
auradb gc --data-dir /data --json     # machine-readable report
```

A server runs GC in the background when `[mvcc] gc_enabled = true`, every
`gc_interval_secs`. GC preserves every version any active transaction can see, so
it is always safe to run. The report includes versions reclaimed, records
removed, versions retained, and bytes reclaimed.

## Health, status, and metrics

`auradb status --addr host:port` (add `--json`) reports the MVCC section: active
transactions, timed-out transactions, oldest snapshot age, retained versions,
cumulative timeouts, the configured timeout, and whether GC is enabled.

`auradb doctor --data-dir /data` (add `--json`) inspects a local data directory
and warns on:

- many active transactions holding snapshots;
- an old oldest-snapshot age (long-lived transaction);
- high retained-version pressure (run `auradb gc`);
- GC disabled;
- transaction timeouts disabled;
- stale planner statistics (run `auradb stats analyze`);
- a failed index consistency check (run `auradb index rebuild`).

Prometheus/JSON metrics exported by a running server include:

| Metric | Meaning |
| ------ | ------- |
| `auradb_mvcc_active_transactions` | transactions holding a pinned snapshot |
| `auradb_mvcc_oldest_snapshot_age_seconds` | age of the oldest snapshot |
| `auradb_mvcc_retained_versions` | stored versions retained |
| `auradb_mvcc_gc_runs_total` | GC passes run |
| `auradb_mvcc_gc_reclaimed_versions_total` | versions reclaimed by GC |
| `auradb_mvcc_gc_reclaimed_bytes_total` | bytes reclaimed by GC |
| `auradb_mvcc_transaction_timeouts_total` | transactions reaped for timeout |
| `auradb_mvcc_conflicts_total` | commit conflicts |

## Backup and restore

AuraDB's logical dump exports the **latest committed visible state**, not the full
version history:

```bash
auradb dump --data-dir /data --out backup.jsonl
auradb restore --data-dir /restored --in backup.jsonl
```

Properties that hold across GC:

- A dump taken after GC restores the same visible latest state.
- A dump taken while a snapshot is held still exports the latest committed state.
- Restore never resurrects versions GC reclaimed: the restored store starts with
  one version per live record.
- Restoring then running GC is safe and idempotent.

Backup and restore are **unaffected by cluster mode**: `dump` and `restore`
operate on the engine's visible state and behave identically whether cluster mode
is on or off.

## Single-node cluster mode (v0.4.0)

Cluster mode is opt-in and **off by default**; the recommended production path
remains single-node, non-cluster mode. To run a single-node cluster — every commit
ordered through a durable Raft log and replayed on restart — enable `[cluster]`
with no peers:

```bash
# Validate the configuration offline first.
auradb config validate --config examples/auradb.cluster.local.toml

# Create node and cluster identity for a data directory.
auradb cluster init --data-dir /data

# Inspect identity and configuration without standing up a node.
auradb cluster status --data-dir /data --json
auradb cluster doctor --data-dir /data
```

`auradb cluster doctor` validates the `[cluster]` configuration and on-disk
identity offline and is the first stop when a cluster node refuses to start. A
single-node cluster provides **no fault tolerance** (same availability as a single
non-cluster node) and adds write-path overhead. Multi-node deployment is
experimental and disabled in this release: configuring peers is rejected at
startup.

The durable Raft log grows over time; compact it (after the engine has applied the
prefix) with `auradb cluster compact-log [--dry-run] [--json]`. Capture and inspect
portable snapshots with `auradb snapshot create|inspect|restore`. For diagnosing
`not_leader`, peer rejection, corrupt cluster metadata, or recovery from backup,
see [CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md). See also
[CLUSTERING.md](CLUSTERING.md) and [CLI.md](CLI.md).

## Upgrading

Upgrading is a drop-in binary replacement; the storage format is unchanged at v2.
A v0.1.0/v0.2.x directory migrates to v2 transparently on first open. An unknown
future format is rejected rather than opened. See [UPGRADING.md](UPGRADING.md).
