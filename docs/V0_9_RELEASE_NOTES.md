# AuraDB v0.9.0 release notes

**HA release candidate for the controlled static-cluster preview.**

AuraDB v0.9.0 moves the static-cluster preview toward a credible
high-availability (HA) **release candidate** by strengthening failure testing,
cluster diagnostics, snapshot/compaction coverage, connector behavior under
leader change, operator recovery runbooks, the cluster backup/restore story, and
the release criteria. It adds **no** new cluster architecture and changes **no**
Raft, storage, query, MVCC, replication, or snapshot semantics except where a
documented bug is fixed.

> **AuraDB v0.9.0 is an HA release candidate for the controlled static-cluster
> preview, not a production HA guarantee. Single-node mode remains the
> recommended production mode.**

It is **not** production HA, not production automatic failover, and not
production cluster readiness. See
[HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) for the support levels, the
operator assumptions, the validated failure matrix, and the strict criteria that
must be met and documented before any future production HA claim.

## Compatibility

- **Storage format unchanged** (manifest `format_version` 2). Data directories
  from every prior release open in place; no migration is required.
- **Aura Wire Protocol unchanged** (AWP 1).
- **Aura Connector v0.4.1 compatible** (and v0.3.x / v0.4.0 as before). The
  connector is unchanged in this release.
- All v0.8.1 behavior is preserved except where a documented bug is fixed.

## Highlights

### HA release-candidate criteria

A new [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) defines exactly what the
candidate does and does not mean: the support level for each mode, the required
operator assumptions (static membership, odd node count, peer TLS and auth,
stable storage, an external supervisor, and backup/restore drills), the
validated failure matrix, what is explicitly not yet production HA, and the
strict, documented criteria required before AuraDB ever claims production HA.

### Cluster failure matrix

A single failure matrix — in [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md),
[CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md), and
[TESTING.md](TESTING.md) — maps each failure (leader/follower kill, old-leader
rejoin, follower lag, snapshot-after-compaction, minority/majority partition,
heartbeat/AppendEntries drop, reconnect storm, TLS/token failure, disk pressure,
snapshot-install failure, backup/restore after failure) to its expected
behavior, the test that covers it, the operator command to observe it, the known
limitation, and its production-HA status.

### Longer repeated fail-stop tests

New `ha_repeated_leader_restart_3_cycles` (CI-safe, required) drives three
kill/elect/rejoin cycles, asserting convergence, no duplicate apply, and that the
leader-change metric increments. `ha_old_leader_rejoins_each_cycle`,
`ha_repeated_restart_no_duplicate_apply`, and `ha_repeated_restart_indices_converge`
pin the individual properties, and `ha_repeated_leader_restart_10_cycles_ignored`
is an `#[ignore]`d ten-cycle stress run for on-demand use.

### Larger snapshot install and compaction tests

New snapshot/compaction coverage:
`ha_snapshot_install_after_compaction_with_offline_follower`,
`ha_snapshot_install_then_more_writes_converges`,
`ha_snapshot_install_preserves_indexed_workload`,
`ha_compaction_with_all_followers_caught_up`,
`ha_compaction_with_offline_follower_requires_snapshot`,
`ha_snapshot_failure_safe_to_retry`, `ha_snapshot_metrics_after_install`, and an
`#[ignore]`d `ha_snapshot_large_ignored_stress`. These confirm an offline
follower catches up by snapshot install after compaction, that indexed /
full-text / document-path / vector records survive the install, that snapshot
install failure is safe to retry, and that snapshot metrics reflect the install.

### Published-image HA smoke

`scripts/smoke_ha_candidate.sh` stands up a three-node Compose cluster from a
built or published image, waits for a leader, writes through it, kills the
leader, waits for a new leader, writes through it, rejoins the old leader, waits
for catch-up, checks cluster status, optionally runs the connector leader-change
scenario, and tears down cleanly (printing logs on failure). It is wired as a
manual / post-release job in [`cluster.yml`](../.github/workflows/cluster.yml)
and is an HA *candidate* smoke, not production HA proof.

### Connector redirect behavior under leader change

A new `tests/conformance/python/run_connector_leader_change.py` validates Aura
Connector v0.4.1 across a leader change: the write to the old leader fails, the
client discovers and reconnects to the new leader, the bounded
`with_leader_redirect()` works without infinite retry, TLS/auth are preserved,
and transactions are not auto-redirected.

### Operator recovery runbooks

[RUNBOOKS.md](RUNBOOKS.md) and
[CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md) gain recovery runbooks
for leader kill, graceful shutdown, no leader, quorum loss, old-leader rejoin,
a follower stuck behind, snapshot needed/failing, peer TLS failure, token
mismatch, reconnect storm, a failed published-image smoke, rolling back a bad
release, and restoring a single-node backup after a cluster incident — each with
symptoms, commands, expected output, safe/unsafe actions, when to restore from
backup, and what to include in a bug report.

### Cluster backup and restore

The leader-logical-export → single-node-restore story is clarified and validated
around failure: backup before and after a leader change, restore the latest
leader state to a single node, the live-cluster restore is refused, and a
restored single node can bootstrap a fresh preview cluster
(`cluster_backup_before_and_after_leader_change`,
`cluster_backup_restore_latest_leader_state`,
`cluster_restore_live_cluster_rejected_or_documented`,
`cluster_restore_to_single_node_then_bootstrap_preview_cluster`).

### GitHub Actions Node 24 maintenance

Workflow actions were updated to majors that run on Node 24, ahead of the Node 20
deprecation: `actions/setup-python` (→ v6) and `actions/upload-artifact` /
`actions/download-artifact` (→ v5). `actions/checkout` (v6), `actions/cache`
(v5), and the `docker/*` actions were already on Node-24 majors. The release
workflow's security posture is unchanged.

## Upgrading

v0.9.0 is a drop-in upgrade over v0.8.1. There is no storage migration. As
always, take a backup and run a restore drill before upgrading a production
deployment; see [UPGRADING.md](UPGRADING.md) and [RUNBOOKS.md](RUNBOOKS.md).
Single-node mode remains the recommended production mode.

## What this release is not

v0.9.0 does **not** claim production clustering, production cluster readiness,
production automatic failover, linearizable follower reads, distributed
transactions, dynamic membership, sharding, multi-region, serializable
isolation, ANN/HNSW, BM25, or hybrid fusion. None of those are present.
Multi-node mode remains a controlled static-cluster preview; single-node mode
remains the recommended production mode. See
[HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md).
