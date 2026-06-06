# AuraDB v0.6.0 release notes

**Theme: Cluster ergonomics and fail-stop recovery preview.**

AuraDB v0.6.0 improves the controlled multi-node **preview** experience and
validates fail-stop recovery behavior: a leader is stopped, the surviving
majority elects a new leader, the new leader accepts writes, and the old node
rejoins as a follower and catches up. It adds the first real **peer snapshot
install over the wire**, sharper fail-stop diagnostics, and operator runbooks
for peer certificate/token rotation and cluster backup/restore.

This release is **not production HA.** It makes no production automatic-failover
claim, and it does not implement linearizable follower reads, distributed
transactions, dynamic membership, sharding, or multi-region — and it claims none
of them. Multi-node mode stays experimental, off by default, and gated behind
two explicit opt-ins. **Single-node mode remains the recommended production
mode.**

It changes no on-disk storage format and no wire protocol (AWP 1), and it
preserves Aura Connector 0.3.x compatibility — no connector release is required.
The cluster health report gains additive fail-stop diagnostics fields that older
clients ignore.

## At a glance

- **Fail-stop recovery preview.** Stopping a leader is taken over by the
  surviving majority, which elects a new leader that accepts writes; the old
  node restarts as a follower and catches up; all nodes converge. This is a
  preview of fail-stop recovery, **not** production automatic failover.
- **Peer snapshot install over the wire.** When a follower has fallen behind the
  leader's compacted log prefix and can no longer be served by AppendEntries, the
  leader now ships a **bounded, single-message** state-machine snapshot over the
  peer transport. The follower validates it (cluster id, manifest format, payload
  digest, last-included index/term, storage format, and a strict size limit),
  installs it atomically into its engine, advances its Raft compaction boundary,
  and resumes AppendEntries. Oversized, wrong-cluster, bad-digest, and
  future-format snapshots are rejected, and a rejected install leaves existing
  follower state untouched. This is a preview transfer (one bounded message, not
  chunked streaming).
- **Connector write recovery.** After a leader kill, a write to a follower (or to
  the old leader) returns a structured `not_leader` error carrying a leader hint
  and a retryable flag; a client that retries against the new leader's address
  succeeds. Aura Connector 0.3.x handles this today with no connector release.
- **Larger follower catch-up.** Additional tests cover a follower that misses a
  long run of committed entries — across transaction batches and a compacted-log
  boundary — replaying its durable log (or installing a snapshot) without
  duplicate application and with MVCC timestamps and indexes preserved.
- **Sharper fail-stop diagnostics.** Cluster diagnostics surface leader-change
  counts, per-peer reachability and replication indices, and snapshot
  install/sent/rejected counters.
- **Operator runbooks.** Peer certificate and token rotation (rolling restart),
  and cluster backup/restore (leader-side logical backup → single-node restore →
  seed a fresh preview cluster).
- **Published-image Docker Compose smoke.** `scripts/smoke_cluster_compose.sh`
  now honors `AURADB_IMAGE`, so the three-node Compose smoke runs against a
  locally built image (`auradb:0.6.0`, the required path) or a published image
  (`ghcr.io/ohswedd/auradb:0.6.0`, verified post-release).

## Peer snapshot install (preview)

The v0.5.x preview retained the full Raft log under a live follower and answered
a snapshot-install request as structured "unsupported". v0.6.0 implements the
first real install:

- **Trigger.** The leader detects a follower whose next index is at or below its
  compacted prefix (it can no longer serve that follower with AppendEntries) and,
  rate-limited per peer, ships a snapshot covering its current committed state.
- **Transfer.** The snapshot is a single `InstallSnapshotRequest` message,
  base64-framed and capped at 8 MiB (`MAX_SNAPSHOT_BYTES`). A dataset whose
  snapshot exceeds that limit is logged and not shipped — the preview does not do
  chunked streaming.
- **Validation.** The follower checks the cluster id, manifest format version,
  payload digest, last-included index/term agreement, storage format version, and
  the size limit **before** mutating any state.
- **Install.** On success the follower installs the snapshot into its live engine
  at the snapshot's commit timestamp, advances its durable Raft boundary
  (discarding the subsumed prefix), and acknowledges; the leader then resumes
  AppendEntries from just past the boundary.
- **Failure safety.** A rejected snapshot (oversized, wrong cluster, bad digest,
  future format) leaves existing follower data untouched.

**Preview limitations.** This is a single-message bounded transfer, not chunked
streaming. It targets the fail-stop case where the follower is strictly behind
the snapshot (it only fell behind); it does not reconcile divergent follower
history. Live log compaction that enables an install is operator-initiated
(`PeerCluster::compact_log`, the live counterpart to `auradb cluster
compact-log`). It is a preview, not a production-grade snapshot subsystem.

## Compatibility

- **Storage format:** unchanged (v2).
- **Aura Wire Protocol:** AWP 1, unchanged. The cluster health section gains
  additive fail-stop diagnostics fields; the optional `retryable` error hint and
  the `not_leader` error code are unchanged.
- **Aura Connector:** 0.3.x remains fully compatible — no connector release is
  required for v0.6.0.
- **Single-node and single-node-cluster behavior:** unchanged.

See [COMPATIBILITY.md](COMPATIBILITY.md) and
[AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md).

## Release validation

In addition to the workspace test suite, fmt/clippy gates, and the artifact
audit, v0.6.0 was validated locally:

- **Docker image** `auradb:0.6.0` builds from the multi-stage `Dockerfile`, and
  `docker run --rm auradb:0.6.0 auradb version` prints `auradb 0.6.0`. All three
  Compose files (`docker-compose.yml`, `.secure.yml`, `.cluster.yml`) validate.
- **Live Docker Compose cluster** (`AURADB_IMAGE=auradb:0.6.0 bash
  scripts/smoke_cluster_compose.sh`) brought up a three-node mutual-TLS cluster,
  elected a leader, reported quorum with both peers connected, and tore down
  cleanly.
- **Published Aura Connector 0.3.0** (installed from PyPI) passed the AWP protocol
  conformance (18/18), the connector smoke (12/12), and the full connector
  conformance (15/15) against a v0.6.0 server — no connector release required.

## Upgrading

v0.6.0 is a drop-in upgrade from v0.5.x for single-node and single-node-cluster
deployments: no data migration, no config change, no connector change. The
multi-node preview remains opt-in (`[cluster] enabled = true` plus
`experimental_multi_node = true`) and static-membership only. See
[UPGRADING.md](UPGRADING.md).

## Known limitations

- Multi-node mode is an experimental preview, not production HA. No production
  automatic failover, no linearizable follower reads, no distributed
  transactions, no dynamic membership, no sharding, no multi-region.
- Peer snapshot install is a bounded single-message transfer (no chunked
  streaming) and targets the strictly-behind follower case.
- Cluster backup is leader-side logical export restored into a single-node data
  directory that can seed a fresh preview cluster; restoring directly into a
  live multi-node cluster is not supported.
- Single-node mode remains the recommended production mode.
