# Backup, restore, and recovery drills

This is the canonical operator guide for backing up, verifying, restoring, and
rolling back a **single-node** AuraDB deployment, and for rehearsing those
operations with the production drill harness.

Scope and honesty:

- Single-node is the **production-supported** deployment mode. Everything here
  targets it.
- Multi-node is an **HA candidate preview only**, not production HA — see
  [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md). Cluster backup/restore
  *planning* helpers exist (`auradb cluster backup-plan` / `restore-plan`) but
  the recovery path is still "restore into a fresh single-node data dir".
- HNSW/ANN vector search is an **opt-in preview only**; exact vector search is
  the correctness baseline. No drill here proves production ANN.

## The two backup forms

| Form | Command | What it captures | Use it for |
| --- | --- | --- | --- |
| Logical dump (JSONL) | `auradb dump` / `auradb restore` | latest committed visible state, one record per primary key | portable backups, migration, recovery into a fresh dir |
| Portable snapshot | `auradb snapshot create` / `snapshot restore` | a self-describing, integrity-checked image of the data dir | fast restore-to-fresh and **rollback to a known-good point** |

A logical dump exports the *latest committed visible state*, not full MVCC
history. Properties that hold across GC are documented in
[OPERATIONS.md](OPERATIONS.md#backup-and-restore).

## Create and verify a backup

```bash
auradb dump --data-dir /var/lib/auradb --out backup.jsonl
# Validate WITHOUT importing — exits non-zero on an invalid backup.
auradb backup verify --input backup.jsonl --json
```

`backup verify` checks that every line parses, the per-line size bound holds,
records reference declared schemas, and there are no duplicate primary keys. The
report carries counts only — never record values or secrets.

## Restore into a fresh data directory

Always restore into a **fresh** directory, then point the server at it. This
leaves the original untouched for forensics.

```bash
# Logical restore:
auradb restore --data-dir /var/lib/auradb-restored --input backup.jsonl

# or portable-snapshot restore (refuses a non-empty target without --force):
auradb snapshot create  --data-dir /var/lib/auradb        --output known-good.snap
auradb snapshot restore --input known-good.snap --data-dir /var/lib/auradb-restored

# Confirm health before cutting over:
auradb check  --data-dir /var/lib/auradb-restored --json   # expect ok == true
auradb doctor --data-dir /var/lib/auradb-restored --json   # expect index_consistency_ok == true
```

## Rollback rehearsal (return to a known-good point)

Before a risky change (upgrade, bulk import), capture a known-good snapshot. If
the change goes wrong, force-restore the snapshot over the data dir:

```bash
auradb snapshot create --data-dir /var/lib/auradb --output known-good.snap
# ... apply the change ...
# If it goes wrong, roll back:
auradb snapshot restore --input known-good.snap --data-dir /var/lib/auradb --force
auradb check --data-dir /var/lib/auradb --json
```

The restore is atomic (built in a staging directory and swapped into place), so
a crash mid-restore never leaves a half-restored data dir.

## The production drill harness

`scripts/smoke_single_node_production_drills.sh` rehearses all of the above plus
disk-headroom preflight and clean I/O-error surfacing, against bounded,
throwaway data under `.local/`. It is a **single-node production drill — not a
multi-node HA proof, and it makes no production ANN claim.**

### When to run

- Before a production release (release gate, run locally / on demand).
- After changing storage, backup, or restore code.
- As a periodic recovery-confidence rehearsal.

### How to run

```bash
scripts/smoke_single_node_production_drills.sh
# Larger state and a stricter free-space threshold:
DRILL_RECORDS=5000 MIN_FREE_MB=1024 scripts/smoke_single_node_production_drills.sh
# Keep the data dirs for inspection (logs + report are always kept):
KEEP_ARTIFACTS=1 scripts/smoke_single_node_production_drills.sh
```

Environment knobs: `DRILL_RECORDS` (default 2000), `MIN_FREE_MB` (soft threshold,
default 500; a 50 MB hard floor fails the preflight), `KEEP_ARTIFACTS`,
`AURADB_BIN` (use a prebuilt binary instead of `cargo build`).

### Drill sections

1. `disk_space_preflight` — read-only `df` headroom check (see below).
2. `backup_restore_rehearsal` — seed a bounded "larger state", dump, and verify.
3. `restore_to_fresh_data_dir` — snapshot and restore into a fresh dir; counts must match.
4. `rollback_rehearsal` — force-restore a known-good snapshot over a divergent state.
5. `io_error_surface_check` — permission-denied / missing / corrupt paths (see below).
6. `doctor_after_restore` — `doctor --json` reports a healthy, consistent state.
7. `check_after_restore` — `check --json` reports `ok == true`.
8. `metrics_or_stats_snapshot` — capture a machine-readable stats snapshot as evidence.

### Expected output and the JSON report

Each drill prints a `[PASS]`, `[WARN]`, or `[FAIL]` line. The script exits
non-zero on any real failure. A machine-readable report is written to
`.local/prod-drills/run-<epoch>/report.json`:

```json
{
  "tool": "smoke_single_node_production_drills",
  "label": "single-node production drill; not a multi-node HA proof; no production ANN claim",
  "overall": "pass",
  "failures": 0,
  "drills": [
    { "name": "backup_restore_rehearsal", "status": "pass", "duration_secs": 3,
      "checks": ["seeded_records=2000", "dump", "backup_verify_ok=True"], "warnings": [] }
  ]
}
```

Read it with `python3 -m json.tool` or `jq`. Top-level fields: `overall`,
`failures`, `data_dir`, `backup_path`, `restore_path`, `snapshot_path`,
`rollback_path`, `run_warnings`, and a `drills[]` array of `{name, status,
duration_secs, checks[], warnings[]}`.

### What a failure means

- `disk_space_preflight` FAIL — free space is below the 50 MB hard floor (or `df`
  failed). WARN means below the soft threshold; add headroom.
- `backup_restore_rehearsal` / `restore_to_fresh_data_dir` / `rollback_rehearsal`
  FAIL — backup, verify, or restore is broken, or a restore lost/duplicated
  records. **Do not release.** Capture the preserved artifacts and the report.
- `io_error_surface_check` FAIL — an I/O fault panicked instead of erroring
  cleanly (a robustness regression). A WARN means the environment could not
  induce the fault (e.g. running as root bypasses permission bits).
- `doctor_after_restore` / `check_after_restore` FAIL — a restored data dir is
  inconsistent. Investigate before trusting the backup path.

### Preserving logs

Logs are **always** kept at `.local/prod-drills/logs/drills_<epoch>.log`. On
failure the data directories are preserved too; on success they are removed
unless `KEEP_ARTIFACTS=1`. Attach the log and `report.json` to any bug report.

### What is NOT proven

The drills prove the **single-node** backup/restore/rollback and recovery path.
They do **not** prove: multi-node replication, failover, or quorum; production
HA; production ANN/recall; or behaviour under a genuinely full disk (the disk
drill is a read-only preflight and never fills the real disk).

## Disk-full drill (safe approach)

Never simulate disk-full by actually filling the disk. The harness instead runs a
read-only `df` preflight that verifies the free-space check and its structured
warning/failure path. To rehearse the alert path, set `MIN_FREE_MB` above the
current free space and confirm the preflight emits a `WARN` and a `run_warnings`
entry.

Remediation checklist for low disk:

1. `auradb gc` to reclaim superseded MVCC versions.
2. `auradb compact` to compact the storage log.
3. Move or expand the volume backing the data directory.
4. Re-run the preflight (or `auradb check --json`) to confirm headroom.

## I/O-error drill (safe approach)

Simulate I/O faults with safe, reversible conditions — never by corrupting real
data:

- **Permission denied:** write a backup into a `chmod 000` directory; expect a
  structured error naming the path, exit non-zero, no panic.
- **Missing path:** restore from a non-existent snapshot/dump; expect a clean error.
- **Corrupt input:** `restore` / `backup verify` a malformed JSONL or snapshot;
  expect rejection with a structured error.

Expected: every fault surfaces as a non-zero exit with a clear message and
**never** a panic. Remediation: fix the path/permissions/volume and retry; if a
backup file itself is corrupt, fall back to the previous verified backup.

## SLO / recovery report template

Track these per environment (the drill report and `check`/`doctor --json`
provide the raw numbers):

| Metric | Source | Target (example) |
| --- | --- | --- |
| Backup age | timestamp of the last verified `dump` | < 24 h |
| Restore-rehearsal recency | last green drill run | < 7 days |
| `doctor` / `check` result | `auradb check --json` `ok`; `doctor --json` `index_consistency_ok` | both true |
| Query timeout rate | `query_timeout` responses / total (see [OPERATIONS.md](OPERATIONS.md)) | < 0.1% |
| Disk headroom | `disk_space_preflight` `df_available_mb` | > soft threshold |

See also: [RUNBOOKS.md](RUNBOOKS.md), [OPERATIONS.md](OPERATIONS.md),
[PRODUCTION_READINESS.md](PRODUCTION_READINESS.md), [TESTING.md](TESTING.md),
[CLI.md](CLI.md).
