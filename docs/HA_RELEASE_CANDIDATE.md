# HA release candidate

> **AuraDB v0.9.0 is an HA release candidate for the controlled static-cluster
> preview, not a production HA guarantee. Single-node mode remains the
> recommended production mode.**

This document defines exactly what the v0.9.0 high-availability (HA) release
candidate **does** and **does not** mean. It is the single place that states the
support level for each deployment mode, the operator assumptions the static
cluster requires, the failure matrix the candidate is validated against, and the
strict criteria that must be met — and documented — before AuraDB ever claims
*production* HA.

v0.9.0 does not change the cluster architecture, the wire protocol (AWP 1), or
the storage format (manifest `format_version` 2). It strengthens failure
testing, cluster diagnostics, snapshot/compaction coverage, connector behavior
under leader change, operator runbooks, the cluster backup/restore story, and
the release criteria. It adds no new cluster features and makes no new
guarantees beyond what the tests below actually demonstrate.

## 1. Support level

| Mode | Support level in v0.9.0 |
| ---- | ----------------------- |
| **Single-node, non-cluster** | **Recommended production mode.** Commits go straight to storage. This is the default and the supported production path. |
| **Single-node cluster** (Raft enabled, no peers) | Supported for validation/development. Exercises the replicated commit path but provides **no** fault tolerance (one node is its own majority). |
| **Static multi-node cluster** (Raft + static peers) | **HA release candidate** — a controlled static-cluster preview. Off by default; gated behind two opt-ins. Validated against the failure matrix in §3. **Not** a production HA guarantee. |

The static multi-node cluster explicitly does **not** provide:

- dynamic membership (no `join` / `leave` / `step-down`; membership is static);
- distributed transactions;
- sharding or multi-region deployment;
- linearizable follower reads (followers reject reads by default);
- an operator-driven automatic-failover SLA.

See [CLUSTERING.md](CLUSTERING.md) for the architecture and
[PRODUCTION_READINESS.md](PRODUCTION_READINESS.md) for the overall posture.

## 2. Required operator assumptions

The HA release candidate is only valid under all of the following operator
assumptions. If any is not met, treat the deployment as unsupported.

- **Static membership.** Every node declares every other node by `node_id` and
  `addr`. Nodes are not added or removed online.
- **Odd number of nodes.** Run 3 (or 5) nodes so a clear majority always exists.
  An even count gives no availability benefit and risks split quorum.
- **Peer TLS.** Any non-loopback cluster networking requires peer TLS
  (`[cluster.tls]` with `cert_path` / `key_path` / `ca_path`). See
  [SECURITY.md](SECURITY.md).
- **Peer authentication.** A shared `peer_auth_token` is required for any
  non-loopback cluster. Unauthenticated public peer traffic is never permitted.
- **Stable storage.** Each node has durable local storage for its data segments
  and its Raft log; disks are monitored for capacity and I/O health.
- **External process supervisor.** A supervisor (systemd, Docker restart policy,
  Kubernetes, etc.) restarts a crashed node. AuraDB does not self-restart.
- **Backups and restore drills.** Logical backups are taken from the leader and
  a restore drill is rehearsed before relying on the cluster. See
  [§5](#5-backup-and-restore) and [RUNBOOKS.md](RUNBOOKS.md).

## 3. HA release-candidate failure matrix

Each row is a failure (or operation) the candidate is validated against, the
behavior an operator should expect, where it is tested, the operator command to
observe it, the known limitation, and its production-HA status. "Tested by"
names the automated coverage; serial cluster tests run with `--test-threads=1`
(see [TESTING.md](TESTING.md)).

| # | Scenario | Expected behavior | Tested by | Operator command | Known limitation | Production-HA status |
| - | -------- | ----------------- | --------- | ---------------- | ---------------- | -------------------- |
| 1 | Leader process killed | Surviving majority elects a new leader; writes resume on it. | `ha_repeated_leader_restart_3_cycles`, `leader_restart_elects_new_leader` | `auradb cluster leader --addr`; `auradb cluster wait-leader --addr` | Re-election latency is not an SLA. | Candidate |
| 2 | Leader graceful shutdown | Same as a kill from the cluster's view; majority re-elects. | `leader_restart_elects_new_leader` | `auradb cluster status --addr --json` | No explicit `step-down`; stop the process. | Candidate |
| 3 | Follower process killed | Majority keeps committing; the follower catches up on restart. | `follower_catches_up_after_restart` | `auradb cluster status --addr --json` (per-peer `match_index`) | None at preview scale. | Candidate |
| 4 | Old leader rejoins | Rejoins as a follower at the current term and catches up. | `ha_old_leader_rejoins_each_cycle`, `old_leader_rejoins_as_follower` | `auradb cluster status --addr --json` | None at preview scale. | Candidate |
| 5 | Follower offline during writes | Majority commits; the offline follower lags, then catches up by log replay or snapshot install. | `follower_catches_up_with_transaction_batches` | `auradb cluster doctor --addr` | Lag is bounded by retained log vs. snapshot. | Candidate |
| 6 | Follower needs snapshot after compaction | Leader serves a snapshot install (single-message, bounded) since the needed entries were compacted. | `ha_snapshot_install_after_compaction_with_offline_follower`, `install_snapshot_restores_follower_after_compaction` | `auradb cluster status --addr --json` (snapshot diagnostics) | Snapshot is a single bounded message (no chunked streaming). | Candidate |
| 7 | Minority partition | The minority side cannot commit (no quorum); it serves no writes. | `minority_partition_leader_write_times_out`, `minority_cannot_commit` | `auradb cluster status --addr --json` (`quorum_available`) | None — this is the safety property. | Candidate |
| 8 | Majority partition | The majority side keeps committing; the minority rejoins and catches up on heal. | `majority_partition_write_succeeds`, `partition_heals_and_follower_catches_up` | `auradb cluster doctor --addr` | None at preview scale. | Candidate |
| 9 | Temporary heartbeat drop | Brief heartbeat loss may trigger a re-election; the cluster reconverges. | `leader_partition_triggers_reelection_and_heals` | `auradb cluster status --addr --json` (`leader_changes`) | Re-election timing is not an SLA. | Candidate |
| 10 | Temporary AppendEntries drop | Replication resumes via log repair once delivery returns. | `partition_heals_and_follower_catches_up` | `auradb cluster doctor --addr` | None at preview scale. | Candidate |
| 11 | Peer reconnect storm | Bounded-backoff reconnects recover replication without duplicate apply. | `peer_reconnect_storm_replication_recovers`, `peer_reconnect_storm_no_duplicate_apply` | `auradb cluster doctor --addr` (reconnect-storm warning) | None at preview scale. | Candidate |
| 12 | TLS peer failure | A wrong-CA/SAN peer is rejected by the handshake; no plaintext fallback. | `peer_tls` (server), `peer_tls_cluster` (CLI) | `auradb config validate --config`; node logs | Cert rotation is a manual, documented drill. | Candidate |
| 13 | Peer token mismatch | A wrong-token peer is rejected with a structured `PeerError`. | `peer_tls` / handshake tests | node logs | Token rotation is a manual, documented drill. | Candidate |
| 14 | Disk-pressure warning | Operator-visible warning; AuraDB does not free space automatically. | resource-limit tests (`limits`) | `auradb doctor --data-dir`; `auradb cluster doctor --addr` | No automatic remediation. | Candidate |
| 15 | Snapshot install failure | A failed install is rejected safely; existing state is preserved and the install is retried. | `ha_snapshot_failure_safe_to_retry`, `install_snapshot_failure_preserves_existing_state` | `auradb cluster status --addr --json` (snapshot counters) | Operator-observable retry only. | Candidate |
| 16 | Backup/restore after failure | A logical backup taken from the (new) leader restores to a single node carrying the latest committed state. | `cluster_backup_before_and_after_leader_change`, `cluster_backup_restore_latest_leader_state` | `auradb dump` / `auradb backup verify` / `auradb restore` | Restore targets a single node, not a live cluster. | Candidate |

"Candidate" means the behavior is validated for the controlled static-cluster
preview but is **not** a production HA guarantee. No row should be read as a
production SLA.

## 4. What is not yet production HA

The following are deliberately **out of scope** for v0.9.0 and must not be
inferred from the matrix above:

- **No dynamic membership.** Membership is static; there is no `join`, `leave`,
  or `step-down`, and no joint consensus.
- **No online membership changes.** Changing the node set means a planned,
  offline reconfiguration.
- **No operator-driven failover SLA.** Re-election happens, but no recovery-time
  or recovery-point objective is promised.
- **No automated backup orchestration.** Backups are operator-driven logical
  exports; there is no scheduler or retention manager.
- **No multi-region deployment.** Single-region, low-latency peers only.
- **No cross-shard or distributed transactions.** Isolation is single-node
  snapshot isolation; there is no sharding.

## 5. Backup and restore

The cluster backup/restore story is a **leader logical export → single-node
restore** path, validated around failure:

1. Take a logical backup from the leader before a planned change (`auradb dump`).
2. If the leader fails, the majority elects a new leader and writes continue.
3. Take a fresh backup from the **new** leader to capture the latest committed
   state.
4. `auradb backup verify` the dump, then `auradb restore` it into a fresh
   **single-node** data directory.
5. Restoring into a *live* multi-node cluster is **not supported**; restore
   targets an offline, fresh data directory. To rebuild a preview cluster,
   restore to a single node and then bootstrap a new static cluster around it.

This is validated by `cluster_backup_before_and_after_leader_change`,
`cluster_backup_restore_latest_leader_state`,
`cluster_restore_live_cluster_rejected_or_documented`, and
`cluster_restore_to_single_node_then_bootstrap_preview_cluster`. See
[OPERATIONS.md](OPERATIONS.md) and [RUNBOOKS.md](RUNBOOKS.md) for the operator
procedures, and the recovery runbooks in [RUNBOOKS.md](RUNBOOKS.md) and
[CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md).

## 6. Connector behavior under leader change

Aura Connector v0.4.1 is the recommended client and is **compatible unchanged**.
Under a leader change:

- A write to the old leader fails once it is no longer leader (`not_leader` /
  connection error); the connector does not silently hang.
- The client discovers the new leader from the `not_leader` hint or by
  re-resolving via `auradb cluster leader --addr`, then reconnects with
  `Client.connect_to_leader(exc)`.
- `Client.with_leader_redirect()` performs a **bounded** redirect (no infinite
  retry) and never silently drops TLS.
- Transactions are **not** auto-redirected across a leader change; the
  application restarts the transaction against the new leader.

This is validated by `tests/conformance/python/run_connector_leader_change.py`
and folded into `scripts/smoke_ha_candidate.sh`. See
[AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md) and
[CONFORMANCE.md](CONFORMANCE.md).

## 7. Published-image HA candidate smoke

`scripts/smoke_ha_candidate.sh` runs an end-to-end HA candidate smoke against a
built or published image:

```sh
AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.9.0 scripts/smoke_ha_candidate.sh
```

It generates dev certs, starts a three-node Compose cluster, waits for a leader,
writes through the leader, stops the leader, waits for a new leader, writes
through the new leader, restarts the old leader, waits for catch-up, checks
cluster status, optionally runs the connector leader-change scenario, tears down
cleanly, and prints logs on failure. It is a manual / post-release check (see
[`.github/workflows/cluster.yml`](../.github/workflows/cluster.yml)), **not** a
PR blocker, and is an HA *candidate* smoke, **not** production HA proof. See
[RELEASE.md](RELEASE.md).

## 8. Strict criteria for any future production HA claim

AuraDB will not claim production HA until **all** of the following are met and
**documented** with evidence. None are expected to be complete in v0.9.0.

1. **Repeated long soak.** Multi-hour/day repeated fail-stop and chaos soak with
   zero data loss and zero duplicate/conflicting apply.
2. **Snapshot install under large state.** Validated catch-up at production
   data sizes, with chunked/streaming transfer if a single bounded message is
   insufficient.
3. **Backup/restore cluster drills.** Rehearsed, timed restore drills with
   documented recovery-point/recovery-time results.
4. **Network partitions across environments.** Partition/heal validated beyond
   loopback: real network namespaces, container networks, and cross-host.
5. **Disk-full and I/O-error behavior.** Defined, tested behavior under a full
   disk and I/O errors, with operator guidance.
6. **Process-supervisor integration.** Documented systemd / Docker /
   Kubernetes restart and health-probe integration.
7. **TLS and token rotation drills.** Rehearsed peer certificate and token
   rotation without downtime where possible, with a documented procedure.
8. **Clear SLOs and non-goals.** Published recovery-time / recovery-point
   objectives and explicit non-goals.
9. **Connector behavior under leader change.** Connector redirect, bounded
   retry, and transaction semantics validated against every supported client.
10. **Operational monitoring and alert thresholds.** Documented dashboards,
    metrics, and alert thresholds (leader changes, replication lag, snapshot
    activity, quorum loss).
11. **External feedback / dogfood period.** A sustained dogfood or external-user
    validation period with the issues it surfaces addressed.

Until every criterion is met and documented, multi-node remains a controlled
static-cluster preview and single-node remains the recommended production mode.
See [ROADMAP.md](ROADMAP.md) and [PRODUCTION_READINESS.md](PRODUCTION_READINESS.md).
