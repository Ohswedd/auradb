# Upgrading

## Upgrade guarantee (v1.0)

AuraDB v1.0.0 supports in-place upgrade from documented v0.x release formats
covered by genuine or representative release fixtures. Operators must take a backup
first and run `auradb check` before and after upgrade.

- **Backup first.** Take a logical dump and verify it without importing:
  `auradb dump` then `auradb backup verify`. Run a restore drill before a
  production upgrade.
- **`auradb check` before and after.** Capture a structured consistency report
  before the swap and again after, and compare.
- **No downgrade guarantee.** AuraDB does not guarantee a newer release's data
  directory can be reopened by an older binary unless a specific downgrade path is
  documented below. A directory written with a newer storage format is rejected by
  an older binary, never silently downgraded.
- **Rollback means restore from backup.** Keep the previous binary and the
  pre-upgrade backup until the upgrade is verified in production.
- **Storage format v2 is frozen for v1.** If a future v1.x release ever changes the
  storage format (only for a safety, corruption, or security issue), the migration
  will be documented here.
- **Fixture coverage is not overstated.** Some v0.x releases share storage format
  v2 and are covered by **representative** fixtures: v0.1.0–v0.2.1 are storage
  format v1, and v0.3.x–v0.9.x share storage format v2, so the v0.3.0 fixture is the
  representative v2 storage fixture for that range (see
  [`tests/fixtures/README.md`](../tests/fixtures/README.md)).

## From v0.9.x to v1.0.0

> **AuraDB v1.0.0 is a single-node production release with a multi-node HA
> candidate preview. It is not production HA; single-node mode is the recommended
> production mode.**

v1.0.0 is a **drop-in** binary replacement from v0.9.x (and any earlier
v2-format release). There is **no storage migration** (format stays at v2,
**frozen for v1**), the wire protocol is unchanged (AWP 1, **frozen for v1**), and
Aura Connector v0.4.1 remains compatible. It changes no semantics and adds no new
cluster architecture; it finalizes the v1.0 support policy, compatibility freezes,
upgrade guarantee, backup/restore release gate, and security review.

```bash
auradb dump --data-dir /var/lib/auradb --output backup-before-1.0.0.jsonl
auradb backup verify --input backup-before-1.0.0.jsonl --json
auradb check --data-dir /var/lib/auradb --json
# Stop the old binary, swap in the v1.0.0 binary, start it, then:
auradb check --data-dir /var/lib/auradb --json
```

**Rollback plan.** v0.9.x and v1.0.0 share the same on-disk and wire formats, so a
v1.0.0 data directory can be reopened by v0.9.x. Keep the pre-upgrade backup; for a
cluster preview, take a backup from the current leader before rolling back (see
[RUNBOOKS.md](RUNBOOKS.md)).

## From v0.8.1 to v0.9.0

> **AuraDB v0.9.0 is an HA release candidate for the controlled static-cluster
> preview, not a production HA guarantee. Single-node mode remains the
> recommended production mode.**

v0.9.0 is a **drop-in** binary replacement for v0.8.1. There is **no storage
migration** (format stays at v2), the wire protocol is unchanged (AWP 1), and Aura
Connector v0.4.1 remains compatible. It changes no semantics and adds no new
cluster architecture — only stronger cluster failure testing, diagnostics,
snapshot/compaction coverage, connector behavior under leader change, operator
recovery runbooks, the cluster backup/restore story, and GitHub Actions Node 24
maintenance. See [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md).

As always, take a backup and rehearse a restore before upgrading a production
deployment:

```bash
auradb dump --data-dir /var/lib/auradb --output backup-before-0.9.0.jsonl
auradb backup verify --input backup-before-0.9.0.jsonl --json
# Stop the old binary, swap in the v0.9.0 binary, start it, then:
auradb check --data-dir /var/lib/auradb --json
```

**Rollback plan.** v0.8.1 and v0.9.0 share the same on-disk and wire formats, so a
v0.9.0 data directory can be reopened by v0.8.1. Keep the pre-upgrade backup. For
a cluster preview, take a backup from the current leader before rolling back (see
[RUNBOOKS.md](RUNBOOKS.md) §18l).

## From v0.8.0 to v0.8.1

> **AuraDB v0.8.1 is a production-readiness stabilization patch. It is not
> production HA; single-node mode remains the recommended production mode.**

v0.8.1 is a **drop-in** binary replacement for v0.8.0. There is **no storage
migration** (format stays at v2), the wire protocol is unchanged (AWP 1), and Aura
Connector v0.4.1 remains compatible. It changes no semantics and adds no product
features — only more backup/restore and resource-limit edge-case coverage,
soak-script and release-artifact-verification improvements, and runbook polish.

As always, take a backup and rehearse a restore before upgrading a production
deployment:

```bash
auradb dump --data-dir /var/lib/auradb --output backup-before-0.8.1.jsonl
auradb backup verify --input backup-before-0.8.1.jsonl --json
# Stop the old binary, swap in the v0.8.1 binary, start it, then:
auradb check --data-dir /var/lib/auradb --json
```

**Rollback plan.** v0.8.0 and v0.8.1 share the same on-disk and wire formats, so a
v0.8.1 data directory can be reopened by v0.8.0. Keep the pre-upgrade backup.

## From v0.7.x to v0.8.0

> **AuraDB v0.8.0 is a production-readiness candidate for single-node and a
> stronger cluster preview. It is not production HA; single-node mode remains the
> recommended production mode.**

v0.8.0 is a **drop-in** binary replacement from any v0.7.x (or earlier v0.x)
release. The on-disk **storage format is unchanged at v2** (manifest
`format_version` 2) and the Aura Wire Protocol stays at **AWP 1**, so a v0.7.x data
directory opens directly with no migration and the Aura Connector is unchanged
(v0.4.1 remains recommended). v0.8.0 adds no new database features; it adds
operability and validation tooling.

Recommended upgrade procedure:

```bash
# 1. Back up the data directory and verify the backup WITHOUT importing it.
auradb dump --data-dir /var/lib/auradb --output backup-before-0.8.0.jsonl
auradb backup verify --input backup-before-0.8.0.jsonl --json

# 2. Capture a structured pre-upgrade consistency report (exits non-zero on failure).
auradb check --data-dir /var/lib/auradb --json

# 3. Stop the old binary, swap in the v0.8.0 binary, and start it.

# 4. Capture a structured post-upgrade consistency report and compare.
auradb check --data-dir /var/lib/auradb --json
```

**New optional surface in v0.8.0:** `auradb check --json` (a structured
consistency report), `auradb backup verify` (validate a dump without importing it),
and the `[limits]` config section (five enforced, configurable resource bounds; a
violation returns a structured `limit_exceeded` error without closing the
connection). See [CLI.md](CLI.md) and [CONFIGURATION.md](CONFIGURATION.md).

**Upgrade coverage.** `crates/auradb-cli/tests/upgrade_to_v0_8_0.rs` runs the
v0.8.0 checklist over **genuine release fixtures spanning v0.1.0 through v0.7.1**,
and rejects an unknown future storage format. v0.1.0–v0.2.1 are storage format v1;
v0.3.x–v0.7.x share storage format v2, so the v0.3.0 fixture is the representative
storage fixture for that range.

**Rollback plan.** v0.7.x and v0.8.0 share the same on-disk and wire formats, so a
v0.8.0 data directory can be reopened by v0.7.x. Keep the pre-upgrade backup; if
`check --json` reports a problem after the swap, stop v0.8.0 and restart v0.7.x
against the same directory (or restore the backup). For the step-by-step procedure,
see [RUNBOOKS.md](RUNBOOKS.md) (runbook 5).

## From v0.6.1 to v0.6.2

> **AuraDB v0.6.2 hardens repeated chaos and larger-state recovery behavior in
> the controlled multi-node preview. It is not production HA. Single-node mode
> remains the recommended production mode.**

v0.6.2 is a patch release and a **drop-in** binary replacement from v0.6.x: no
data migration, no config change, and no connector change. Storage stays at v2 and
the Aura Wire Protocol stays at AWP 1. The only wire change is the additive
`leader_changes` field on the cluster health report, which older clients ignore;
Aura Connector 0.3.x remains fully compatible.

If you do nothing, behavior is unchanged. What is new in v0.6.2:

- **Recovery diagnostics.** `auradb cluster status --addr` now reports
  `leader_changes` (a cumulative leadership-instability signal), and
  `auradb cluster doctor --addr` adds two warnings: a peer **reconnect storm**
  (a peer still disconnected after many connection attempts) and **repeated
  leader changes**.
- **Hardened recovery test coverage** (repeated leader restart, larger
  multi-model recovery, multi-model snapshot install, reconnect storms, and
  network-interruption partition/heal simulations). These are tests and
  diagnostics only — there is no behavior change to the running server.

**Downgrade.** v0.6.1 and v0.6.2 share the same on-disk and wire formats, so a
v0.6.2 data directory can be reopened by v0.6.1. As always, back up the data
directory first.

## From v0.6.0 to v0.6.1

> **AuraDB v0.6.1 hardens snapshot install and published-cluster smoke for the
> controlled multi-node preview. It is not production HA. Single-node mode remains
> the recommended production mode.**

v0.6.1 is a patch release and a **drop-in** binary replacement from v0.6.0: no
data migration, no config change, and no connector change. Storage stays at v2
and the Aura Wire Protocol stays at AWP 1; the new diagnostics fields are additive
and ignored by older clients. The peer snapshot-install wire transfer is unchanged
from v0.6.0. Stop the old binary, swap in the new one, start it; for a multi-node
preview cluster, roll one node at a time, keeping a quorum.

If you do nothing, behavior is unchanged. What is new in v0.6.1:

- **Snapshot/lag diagnostics.** `auradb cluster status --addr` gains per-peer
  `lag_entries`, `needs_snapshot`, `snapshot_in_progress`, and `catch_up_state`
  plus cluster-level snapshot diagnostics; a new live `auradb cluster doctor
  --addr` warns on a follower needing a snapshot, a lagging follower, and quorum
  at the minimum / lost; and five new `auradb_cluster_snapshot_*` metrics are
  exported. See [OBSERVABILITY.md](OBSERVABILITY.md).
- **Backup/restore dry-run planners.** `auradb cluster backup-plan` and
  `auradb cluster restore-plan` inspect and report only (they never write data).
  See [OPERATIONS.md](OPERATIONS.md) and [CLI.md](CLI.md).
- **Multi-arch images now published.** The release builds and pushes a
  `linux/amd64` + `linux/arm64` manifest to `ghcr.io/ohswedd/auradb:0.6.1` and
  `:latest`, so `docker pull` selects arm64 automatically on Apple Silicon. Verify
  with `docker buildx imagetools inspect ghcr.io/ohswedd/auradb:0.6.1`.

Aura Connector 0.3.x remains compatible — **no connector release is required**.

**Downgrade.** v0.6.0 and v0.6.1 share the same on-disk and wire formats, so a
v0.6.1 data directory can be reopened by v0.6.0. As always, back up the data
directory before changing versions. See [CLUSTERING.md](CLUSTERING.md).

## From v0.5.x to v0.6.0

> **AuraDB v0.6.0 improves the controlled multi-node preview and validates
> fail-stop recovery. It is _not_ production HA. Single-node mode remains the
> recommended production mode.**

v0.6.0 is a drop-in binary replacement from any v0.5.x release (v0.5.0, v0.5.1,
or the v0.5.2 cert fix). It changes **no on-disk format**: storage stays at v2,
cluster metadata at v1, the Raft log, hard state, compaction marker, commit base,
and the snapshot manifest (v1) are unchanged. A v0.5.x data directory —
single-node or a single-node / multi-node cluster directory with its `cluster/`
identity and Raft state — opens directly with no migration. Stop the old binary,
swap in the new one, start it. For a multi-node preview cluster, roll one node at
a time, keeping a quorum.

If you do nothing, behavior is unchanged. What is new in v0.6.0:

- **Fail-stop recovery preview.** Stopping a leader is taken over by the surviving
  majority, which elects a new leader that accepts writes; the old node rejoins as
  a follower and catches up. This is preview behavior, **not** production
  automatic failover.
- **Peer snapshot install over the wire.** A follower that has fallen behind the
  leader's compacted prefix is brought current by a bounded, single-message
  snapshot install (validated for cluster id, format, digest, boundary, storage
  format, and size). See [REPLICATION.md](REPLICATION.md).
- **Additive diagnostics.** New `auradb_cluster_snapshots_{sent,installed,rejected}_total`
  metrics and a published-image Docker Compose smoke (`AURADB_IMAGE`). The Aura
  Wire Protocol stays at AWP 1 with additive fields only, so Aura Connector 0.3.x
  remains compatible — **no connector release is required**.
- **Cluster backup/restore runbook.** Leader-side logical backup (`auradb dump`)
  restores into a single-node data directory that can seed a fresh preview
  cluster; restoring directly into a live multi-node cluster is not supported. See
  [OPERATIONS.md](OPERATIONS.md).

**Note on v0.5.2.** v0.5.2 was a patch on top of v0.5.1 that fixed the
development certificates generated for the multi-node preview (they now allow both
server and client authentication, which the peer transport's mutual TLS
requires). If you generated dev certs with v0.5.1 for a TLS preview cluster,
regenerate them with `examples/cluster/generate-dev-certs.sh`. No data or wire
change. See the [CHANGELOG](../CHANGELOG.md).

**Downgrade.** v0.5.x and v0.6.0 share the same on-disk and wire formats, so a
v0.6.0 data directory can be reopened by v0.5.x. As always, back up the data
directory before changing versions. See [CLUSTERING.md](CLUSTERING.md).

## From v0.5.0 to v0.5.1

> **AuraDB v0.5.1 hardens the controlled multi-node preview. Single-node mode
> remains the recommended production mode.**

v0.5.1 is a patch release and a drop-in binary replacement. It changes **no
on-disk format**: storage stays at v2, cluster metadata at v1, the Raft log,
hard state, compaction marker, and commit base are unchanged, and the snapshot
manifest stays at v1. A v0.5.0 data directory — single-node or a single-node /
multi-node cluster directory with its `cluster/` identity and Raft state — opens
directly with no migration. Stop the old binary, swap in the new one, start it.

If you do nothing, behavior is unchanged. What is new in v0.5.1:

- `auradb cert generate-dev` accepts `--server-name` and repeatable `--san`
  flags for per-node development certificates; the previous no-argument form is
  unchanged (it still emits a `localhost` / `127.0.0.1` server certificate).
- `auradb cluster status --addr <server>` queries a running server for live
  cluster diagnostics (role, leader, quorum, indices, per-peer reachability).
  The offline `auradb cluster status --data-dir <dir>` form is unchanged.
- The health report's `cluster` section gains additive diagnostics fields and
  the error payload gains an optional `retryable` hint. Both are additive and
  ignored by older clients, so the Aura Wire Protocol stays at AWP 1 and Aura
  Connector 0.3.x remains compatible — no connector release is required.
- `examples/cluster/generate-dev-certs.sh`, `docker-compose.cluster.yml`, and
  `scripts/smoke_cluster_compose.sh` make the local Docker cluster preview
  runnable without hand-crafting certificates. Generated certificates are
  development-only.

**Downgrade.** v0.5.0 and v0.5.1 share the same on-disk and wire formats, so a
v0.5.1 data directory can be reopened by v0.5.0. As always, back up the data
directory before changing versions. See [CLUSTERING.md](CLUSTERING.md).

## From v0.4.1 to v0.5.0

> **AuraDB v0.5.0 introduces a controlled, experimental multi-node server
> preview. Single-node mode remains the recommended production mode.**

v0.5.0 is a drop-in binary replacement and changes **no on-disk format**: storage
stays at v2, cluster metadata at v1, the Raft log and hard state are unchanged,
and the snapshot manifest stays at v1. A v0.4.x data directory — including a
v0.4.x single-node cluster directory with its `cluster/` identity, `raft-log.bin`,
`raft-state.json`, `raft-compaction.json`, and `commit-base.json` — opens directly
with no migration. Stop the old binary, swap in the new one, and start it.

If you do nothing, behavior is unchanged: cluster mode stays off by default and
the single-node path is byte-for-byte the same.

What is new and how it affects an upgrade:

- The `[cluster]` table gains v0.5.0 fields: `experimental_multi_node`,
  `allow_experimental_public_cluster`, structured `{ node_id, addr }` `peers`, a
  `peer_auth_token`, and a `[cluster.tls]` block. All default to the safe,
  disabled state, so an existing config keeps its meaning.
- **Enabling the multi-node preview on upgraded data works.** With both opt-ins
  (`enabled = true` and `experimental_multi_node = true`) and a static `peers`
  list, an upgraded node joins a real cross-process cluster; existing data and
  cluster identity are read normally and the Raft log continues from where it was.
  A non-empty `peers` list **without** `experimental_multi_node = true` is rejected
  at startup (the v0.4.1 behavior), and any non-loopback cluster address fails
  closed unless `allow_experimental_public_cluster = true` (which additionally
  requires peer TLS and a token).
- The health report and `auradb status` gain additive cluster fields
  (`preview_multi_node`, `quorum_available`, and a per-peer `peers` array). The
  Aura Wire Protocol is unchanged at AWP 1, so Aura Connector 0.3.x stays
  compatible — no connector release is required.

**Downgrade.** v0.4.x and v0.5.0 share the same on-disk formats, so a v0.5.0 data
directory can be reopened by v0.4.1 (a v0.5.0-only `[cluster]` field is simply not
acted on by the older binary). As always, back up the data directory before
changing versions. See [CLUSTERING.md](CLUSTERING.md).

## From v0.4.0 to v0.4.1

v0.4.1 is a patch release and a drop-in binary replacement. It changes **no
on-disk format**: storage stays at v2, cluster metadata at v1, the Raft log and
hard state are unchanged, and the snapshot manifest stays at v1 (the new manifest
fields are additive and optional). A v0.4.0 data directory — including a v0.4.0
single-node cluster directory with its `cluster/` identity, `raft-log.bin`,
`raft-state.json`, and `commit-base.json` — opens directly with no migration.

The Raft log gains an optional `raft-compaction.json`, written only the first time
you run `auradb cluster compact-log`; a directory without it is treated as having
an empty compacted prefix. Stop the old binary, swap in the new one, and start it.
Downgrading back to v0.4.0 is safe as long as you have not compacted the Raft log
(v0.4.0 does not read `raft-compaction.json`).

## From v0.3.1 to v0.4.0

v0.4.0 is a drop-in binary replacement. The on-disk **storage format is unchanged
at v2**, so a v0.3.1 data directory opens directly with no migration. Stop the old
server, install v0.4.0, and start it against the same data directory.

v0.4.0 adds optional **cluster (Raft) mode**, which is **off by default**. If you
do nothing, the engine uses the same single-node direct write path as v0.3.1 —
byte for byte — and nothing about your deployment changes.

What is new and how it affects an upgrade:

- The `[cluster]` configuration table is added, `enabled = false` by default. A
  disabled `[cluster]` table is inert and never affects single-node behavior.
- Enabling a single-node cluster (`[cluster] enabled = true`, no peers) on an
  upgraded data directory works: on first start the node creates its cluster/node
  identity under `<data_dir>/cluster/`, elects itself leader, and orders subsequent
  commits through the durable Raft log. Existing data is read normally. Note that a
  single-node cluster provides no fault tolerance.
- Multi-node deployment is experimental and not enabled: configuring `peers` is
  rejected at startup, and a non-loopback cluster bind is rejected without
  `--allow-insecure-bind`.
- The health report and `auradb status` gain an additive `cluster` section, and a
  new `not_leader` error code is additive. The Aura Wire Protocol is unchanged at
  AWP 1, so Aura Connector 0.3.x stays compatible — no connector release is
  required.

**Downgrade.** v0.3.1 and v0.4.0 share storage format v2, so a v0.4.0 data
directory whose cluster mode was never enabled can be reopened by v0.3.1. The
`cluster/` directory is ignored by a non-cluster build. As always, back up the
data directory before changing versions. See [CLUSTERING.md](CLUSTERING.md).

## From v0.3.0 to v0.3.1

v0.3.1 is a drop-in binary replacement. The on-disk **storage format is unchanged
at v2**, so a v0.3.0 data directory opens directly with no migration. Stop the old
server, install v0.3.1, and start it against the same data directory.

What is new and how it affects an upgrade:

- New `[mvcc]` settings `transaction_timeout_secs` (default 300) and
  `abandoned_transaction_reaper_secs` (default 30) take safe defaults if absent. An
  idle transaction is now reaped after the timeout; set `transaction_timeout_secs = 0`
  to preserve the old never-timeout behavior (not recommended).
- The health report and `auradb status` gain an additive `mvcc` section, and
  `EXPLAIN ANALYZE` gains additive diagnostic fields. Aura Connector 0.3.x ignores
  both and stays compatible — no connector release is required.
- A new `transaction_timeout` error code is additive; a connector that does not
  model it falls back to a generic server error.

**Downgrade restriction.** Because v0.3.0 and v0.3.1 share storage format v2, a
v0.3.1 data directory can be reopened by v0.3.0. However, AuraDB never silently
downgrades a storage format, and a directory written by a *newer* format than a
binary understands is rejected on open rather than opened. Always back up the data
directory before changing versions.

## From v0.1.0, v0.2.0, or v0.2.1 to v0.3.0

AuraDB 0.3.0 introduces MVCC, which moves the on-disk storage format from **v1 to
v2** (commit-timestamped version chains; the `auradb-storage` manifest
`FORMAT_VERSION` is `2`). A v1 data directory is **migrated to v2 transparently**
the first time v0.3.0 opens it. Stop the old server, **back up the data
directory**, install v0.3.0, and start it against the same data directory.

When v0.3.0 opens a v1 directory it:

1. Detects `format_version: 1` and migrates the store to v2 in place: each
   existing record becomes the first committed version on its chain (with an
   initial commit timestamp), and the manifest gains `last_commit_ts`. Tombstones
   and version chains are now tracked for all subsequent writes.
2. Initializes planner statistics (`planner_stats.json`) from the migrated data;
   run `auradb stats analyze` afterward to refresh cardinality. Missing or corrupt
   statistics are advisory and fall back to live estimates.
3. Loads persisted index snapshots when present and valid, and otherwise rebuilds
   indexes from storage.
4. Rejects an **unknown future** `format_version` rather than opening it (no
   silent downgrade).

This is validated by `crates/auradb/tests/upgrade_v0_2_x.rs` against committed
v0.2.0 and v0.2.1 fixtures written by the respective release binaries (see
[`tests/fixtures/README.md`](../tests/fixtures/README.md)). The test confirms the
directory migrates and opens, the catalog and records load, version chains and
statistics are initialized, lookups work, and `auradb check` passes.

```bash
# 1. Back up the existing data directory.
auradb dump --data-dir /var/lib/auradb --output backup-before-0.3.0.jsonl

# 2. Install v0.3.0 (replace the binary or pull the new image).

# 3. Open the directory (the v1-to-v2 migration runs on first open) and validate.
auradb check --data-dir /var/lib/auradb
auradb stats analyze --data-dir /var/lib/auradb

# 4. Start the server.
auradb server --config /etc/auradb/auradb.toml
```

> Because 0.3.0 rewrites the storage format to v2, an older binary cannot open a
> directory after it has been migrated. Keep the backup taken before upgrading.

## From v0.1.0 or v0.2.0 to v0.2.1

The on-disk storage format is unchanged across v0.1.0, v0.2.0, and v0.2.1 (the
`auradb-storage` manifest `FORMAT_VERSION` is `1`). Upgrading is therefore a
drop-in binary replacement: stop the old server, install v0.2.1, and start it
against the same data directory.

When v0.2.1 opens an older data directory it:

1. Validates the manifest and rejects an unknown or future `format_version`
   rather than opening it (no silent downgrade).
2. Loads the schema catalog.
3. Loads records from the storage segments.
4. Loads persisted index snapshots when present and valid, and otherwise rebuilds
   indexes from storage. A v0.1.0 directory has no persisted indexes (index
   persistence arrived in v0.2.0), so opening a v0.1.0 directory rebuilds every
   index from storage.

This is validated by `crates/auradb/tests/upgrade_v0_1_0.rs` against a committed
v0.1.0 fixture written by the v0.1.0 binary (see
[`tests/fixtures/README.md`](../tests/fixtures/README.md)). The test confirms the
directory opens, the catalog and records load, indexes rebuild, equality lookups
work against the rebuilt indexes, `auradb check` passes, and a backup taken after
the upgrade round-trips.

## Recommended steps

```bash
# 1. Back up the existing data directory.
auradb dump --data-dir /var/lib/auradb --output backup-before-upgrade.jsonl

# 2. Install v0.2.1 (replace the binary or pull the new image).

# 3. Validate the upgraded directory.
auradb check --data-dir /var/lib/auradb
auradb index check --data-dir /var/lib/auradb

# 4. Optionally rebuild and persist fresh index snapshots.
auradb index rebuild --data-dir /var/lib/auradb

# 5. Start the server.
auradb server --config /etc/auradb/auradb.toml
```

Taking a dump before upgrading is good practice even though no format migration
is required.

## Downgrade

Downgrading to an older binary is supported only while the storage format is
unchanged, which is the case across v0.1.0 through v0.2.1. A directory that a
future AuraDB release wrote with a newer `format_version` will be rejected by an
older binary rather than opened, so always keep a backup before upgrading to a
release that changes the format.

## Security and configuration changes

v0.2.1 preserves all v0.2.0 behavior. There are no breaking configuration
changes. New optional surface:

- `auradb auth rotate-token` for rotating the static token in place.
- `auradb config validate --no-file-checks` for validating a deployment template
  whose TLS files live on the target host.
- `--json` output for `auradb status`, `auradb doctor`, and `auradb bench`.

See [SECURITY.md](SECURITY.md), [CONFIGURATION.md](CONFIGURATION.md), and
[CLI.md](CLI.md).
