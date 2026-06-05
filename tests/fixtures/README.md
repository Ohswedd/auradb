# Test fixtures

## `v0_1_0_data/`

A small AuraDB **v0.1.0** data directory used by the upgrade test
(`crates/auradb/tests/upgrade_v0_1_0.rs`). It contains a manifest
(`format_version: 1`), a schema catalog, and one storage segment with 3 `Org`
records and 10 `User` records. The `User` collection exercises scalar, document,
vector, and relationship fields plus a secondary index on `name`.

It was written by the AuraDB v0.1.0 binary so the upgrade test runs against data
produced by the real v0.1.0 storage engine, not a current-version directory
relabelled as v0.1.0. v0.1.0 did not persist indexes, so the directory has no
`indexes/` snapshot; opening it with v0.2.x rebuilds every index from storage.

### How it was generated

From a checkout of the `v0.1.0` tag:

```bash
git worktree add /tmp/auradb-v010 v0.1.0
# Build a dataset with v0.1.0 storage code and write it to the fixture path.
# (The generator uses the v0.1.0 Engine API: create_schema + insert + flush.)
cargo run -p auradb --example gen_fixture tests/fixtures/v0_1_0_data
```

The storage format (`auradb-storage` manifest `FORMAT_VERSION`) is unchanged
between v0.1.0 and v0.2.1, so the upgrade is a compatibility-validation and
index-rebuild path rather than a data migration. The test also asserts that a
manifest carrying an unknown future `format_version` is rejected rather than
silently opened.

## `v0_2_0_data/` and `v0_2_1_data/`

Small AuraDB **v0.2.0** and **v0.2.1** data directories used by the MVCC upgrade
test (`crates/auradb/tests/upgrade_v0_2_x.rs`). Each was written by the
corresponding release binary, so it carries the real v0.2.x on-disk layout:
storage **format v1** (manifest `format_version: 1`), a schema catalog, storage
segments with records, and persisted index snapshots under `indexes/`.

These fixtures validate the **v1-to-v2 MVCC migration**: opening either directory
with the v0.3.0 engine migrates the store to format v2 transparently (existing
records become the first committed version on their chains and planner statistics
are initialized), after which the catalog and records load, lookups work, and
`auradb check` passes. As with the v0.1.0 fixture, a manifest carrying an unknown
future `format_version` is rejected rather than silently opened.

## `v0_3_0_data/`

A small AuraDB **v0.3.0** data directory used by the upgrade test
(`crates/auradb/tests/upgrade_to_v0_3_1.rs`). It carries the real v0.3.0 on-disk
layout: storage **format v2** (MVCC; manifest `format_version: 2`), a schema
catalog, and one segment holding 5 `Doc` records (scalar, int, and full-text
fields). v0.3.0 and v0.3.1 share the storage format, so this upgrade is a
compatibility-validation path rather than a data migration.

### How it was generated

From a checkout of the `v0.3.0` tag, the v0.3.0 release binary restored a logical
JSONL dump (schema + records) into the fixture path:

```bash
git worktree add /tmp/auradb-v030 v0.3.0
cargo build -p auradb-cli            # build the v0.3.0 binary
./target/debug/auradb restore --data-dir tests/fixtures/v0_3_0_data \
    --input dump.jsonl               # written by the real v0.3.0 storage engine
```

The directory is therefore written by the genuine v0.3.0 engine, not a
current-version directory relabelled as v0.3.0. v0.3.0 `restore` does not persist
index snapshots, so the directory has no `indexes/`; opening it rebuilds every
index from storage.

## Upgrade coverage into v0.3.1

`crates/auradb/tests/upgrade_to_v0_3_1.rs` opens all four fixtures with the
current engine and runs a full checklist: data and schema open, indexes load or
rebuild, planner statistics initialize, the MVCC format is v2, transactions begin
and read a snapshot, the transaction-timeout reaper works, GC runs, and a
backup/restore round-trips. It also asserts that an unknown future
`format_version` is rejected (no silent downgrade).
