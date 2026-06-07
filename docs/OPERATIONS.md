# Operations

Running AuraDB in production: MVCC version pressure, transaction lifecycle,
garbage collection, backup, and the signals that tell you the store is healthy.
This is single-node operations, which is the **recommended production mode**.
AuraDB v0.5.0 adds a controlled, experimental multi-node server preview (see
[Multi-node preview operations](#multi-node-preview-operations-v050)); it is off
by default and not a production path.

v0.8.0 is a production-readiness candidate for single-node operations. For
single-node production guidance and step-by-step operator runbooks, see
[PRODUCTION_READINESS.md](PRODUCTION_READINESS.md) and [RUNBOOKS.md](RUNBOOKS.md).
Three operability tools introduced in v0.8.0 are referenced throughout this guide:
`auradb check --json` (a structured consistency report that exits non-zero on
failure — schedule it to detect storage/catalog/index/stats/raft/snapshot
problems), `auradb backup verify --input <file> --json` (validate a JSONL dump
without importing it), and the `[limits]` config section (five enforced,
configurable resource bounds; a violation returns a structured `limit_exceeded`
error without closing the connection). See [CLI.md](CLI.md) and
[CONFIGURATION.md](CONFIGURATION.md).

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
auradb backup verify --input backup.jsonl --json   # validate WITHOUT importing
auradb restore --data-dir /restored --in backup.jsonl
```

`auradb backup verify` (v0.8.0) validates a dump without importing it: every line
parses, a per-line size bound holds, and records reference declared schemas. It
exits non-zero on an invalid backup, so run it after `dump` and before relying on
the backup. See [CLI.md](CLI.md).

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
`not_leader`, peer state, corrupt cluster metadata, or recovery from backup,
see [CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md). See also
[CLUSTERING.md](CLUSTERING.md) and [CLI.md](CLI.md).

## Multi-node preview operations (v0.5.0, hardened in v0.5.1, fail-stop recovery in v0.6.0)

> **AuraDB v0.6.0 improves the controlled multi-node preview and validates
> fail-stop recovery. It is _not_ production HA. Single-node mode remains the
> recommended production mode.**

The preview lets real server processes form a cross-process cluster, elect a
leader, and replicate writes through Raft. It is **off by default** and requires
two explicit `[cluster]` opt-ins (`enabled = true` and `experimental_multi_node =
true`). The validated path is the three-node loopback example at
`examples/cluster/node{1,2,3}.toml`:

```bash
# Three processes (three terminals):
auradb server --config examples/cluster/node1.toml
auradb server --config examples/cluster/node2.toml
auradb server --config examples/cluster/node3.toml

# Wait for readiness/election and inspect per-peer state.
auradb cluster wait-ready  --addr 127.0.0.1:7171 --timeout-secs 30
auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30
auradb cluster leader      --addr 127.0.0.1:7171 --json
# v0.5.1: live cluster diagnostics (role, leader + client address, quorum,
# indices, replication lag, per-peer reachability and connection attempts).
auradb cluster status      --addr 127.0.0.1:7171 --json
auradb status              --addr 127.0.0.1:7171 --json
```

Or run the whole loopback flow with `scripts/smoke_cluster_loopback.sh`, and the
Docker Compose flow (generate dev certs, start, wait, status, tear down) with
`scripts/smoke_cluster_compose.sh`.

Operating notes:

- **Route writes to the leader.** Followers reject writes with `not_leader` (and
  reject reads). The write path blocks until a majority commits; a minority
  cannot commit, so keep a majority of nodes running.
- **Follower restart.** A restarted follower replays its durable log and the
  leader brings it current; watch `match_index` in `auradb status --json`.
- **Public (non-loopback) clusters fail closed** unless
  `allow_experimental_public_cluster = true`, which additionally requires peer
  TLS (`[cluster.tls]`) and a `peer_auth_token`. The Docker Compose preview under
  `examples/cluster/docker/` shows this path. Generate development certificates
  with `examples/cluster/generate-dev-certs.sh` (development only).
- **Rotate peer certificates and the peer token with a rolling restart**, one
  node at a time, keeping a quorum. Re-issue a node's certificate from the same
  CA and restart only that node; to rotate the CA, distribute an old+new CA
  bundle, roll onto new-CA certs, then drop the old CA. Update `peer_auth_token`
  on each node and restart it, watching `auradb cluster status --addr` for peer
  connectivity. See [SECURITY.md](SECURITY.md) and
  [CLUSTERING.md](CLUSTERING.md).
- **Watch quorum.** `auradb_cluster_quorum_available` and the
  `quorum_available` status field tell you whether a majority is connected.

### Fail-stop recovery (v0.6.0, preview)

Stopping a leader is recovered by the surviving majority — a **fail-stop recovery
preview**, not production automatic failover:

- The majority elects a new leader (term advances); the new leader accepts writes.
- A client that wrote to the old leader gets a `not_leader` error with a leader
  hint and a retryable flag; retry against the new leader's client address
  (`auradb cluster leader --addr <any-node>`).
- The old node restarts as a follower and catches up — by AppendEntries if its
  log still overlaps the leader's, or by a **snapshot install** if it fell behind
  the leader's compacted prefix (see [REPLICATION.md](REPLICATION.md)).
- Track recovery with `auradb cluster status --addr` (role, leader, per-peer
  `match_index`) and the metrics `auradb_raft_leader_changes_total`,
  `auradb_cluster_snapshots_sent_total`, and
  `auradb_cluster_snapshots_installed_total`.

### Cluster backup and restore (v0.6.0, preview)

The preview cluster backup story is **leader-side logical backup → single-node
restore → seed a fresh preview cluster**:

```bash
# 1. Back up the latest committed state from the leader's data directory
#    (stop or quiesce the leader, or run against its data dir).
auradb dump --data-dir /path/to/leader/data --out cluster-backup.jsonl

# 2. Restore into a fresh single-node data directory.
auradb restore --data-dir restored --input cluster-backup.jsonl

# 3. Run restored as single-node (recommended), or bootstrap a NEW preview
#    cluster from it by initializing fresh cluster identities on each node.
auradb server --data-dir restored --bind 127.0.0.1 --port 7171
```

The backup captures committed state only (no uncommitted entries) and excludes
the Raft log. **Restoring directly into a live multi-node cluster is not
supported** — restore into single-node, then bootstrap a fresh preview cluster if
you need one. See [UPGRADING.md](UPGRADING.md) and [STORAGE_ENGINE.md](STORAGE_ENGINE.md).

### Cluster backup and restore dry-run (v0.6.1)

v0.6.1 adds two **dry-run planners** that inspect and report — they **never write
data**:

```bash
# Inspect a data dir + config and report what a logical backup would capture.
auradb cluster backup-plan --data-dir /path/to/data [--config <f>] [--json]

# Inspect a JSONL logical dump and report what a restore would do.
auradb cluster restore-plan --input cluster-backup.jsonl [--json]
```

- `cluster backup-plan` reports the source mode
  (`leader-logical-backup` for a cluster node, else
  `local-data-dir-logical-backup`); what is **included** (latest committed state,
  schema, collection and record counts; indexes rebuilt on restore); what is
  **excluded** (the Raft log and compaction state, cluster membership/peer
  metadata, uncommitted entries, historical MVCC versions); the restore target
  (single-node restore into a fresh data dir, optionally bootstrapping a fresh
  single-node preview cluster); referenced secrets (auth token, peer auth token,
  TLS material) shown **redacted** and noted as **not** included in the backup;
  and warnings (cannot restore directly into a live multi-node cluster; run from a
  stable leader with writes quiesced; verify after restore).
- `cluster restore-plan` inspects a JSONL logical dump and reports its
  schema/record counts, collections, restore target, exclusions, and the same
  "no live multi-node restore" warning.

These planners describe the existing logical backup/restore flow above; they do
not change it. See [CLI.md](CLI.md) and [SECURITY.md](SECURITY.md).

### Multi-arch images and published-image smoke (v0.6.1)

The release-tag workflow builds and pushes a multi-arch (`linux/amd64` +
`linux/arm64`) manifest to `ghcr.io/ohswedd/auradb:0.6.1` and `:latest`, so a
`docker pull` on Apple Silicon selects the arm64 image automatically. Verify the
manifest with `docker buildx imagetools inspect ghcr.io/ohswedd/auradb:0.6.1`. A
manual `published-image-smoke` CI job inspects the manifest and then runs
`scripts/smoke_cluster_compose.sh`, which prints the image used, node ports, the
leader, quorum, per-peer states, and the teardown result (and honors
`AURADB_IMAGE`).

Single-node mode remains the recommended production path; use the preview for
local testing and early validation only. See [CLUSTERING.md](CLUSTERING.md),
[OBSERVABILITY.md](OBSERVABILITY.md), and
[CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md).

### Recovery diagnostics (v0.6.2)

For recovery-heavy scenarios, watch two new signals:

- **`leader_changes`** in `auradb cluster status --addr` — a cumulative count
  that climbs when leadership flaps. A steadily rising value on an otherwise quiet
  cluster means leadership instability, not a single clean failover.
- **`auradb cluster doctor --addr` warnings** — in addition to the existing
  snapshot-needed / lagging-follower / quorum warnings, it now warns on a **peer
  reconnect storm** (a peer still disconnected after many connection attempts) and
  on **repeated leader changes**.

See [CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md) for what each warning
points at and how to resolve it. v0.6.2 hardens repeated chaos and larger-state
recovery in the preview but adds no production-HA claim; **single-node mode
remains the recommended production mode.**

## Upgrading

Upgrading is a drop-in binary replacement; the storage format is unchanged at v2.
A v0.1.0/v0.2.x directory migrates to v2 transparently on first open. An unknown
future format is rejected rather than opened. See [UPGRADING.md](UPGRADING.md).
