# Upgrading

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
