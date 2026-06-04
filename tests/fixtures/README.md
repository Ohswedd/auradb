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
