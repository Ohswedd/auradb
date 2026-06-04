# Upgrading

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
