# AuraDB v0.6.1 release notes

**Theme: Snapshot install and published-cluster smoke hardening.**

AuraDB v0.6.1 is a patch release that makes the v0.6.0 controlled multi-node
**preview** more reliable, observable, and repeatable. It adds
multi-architecture Docker images, larger and concurrent-write snapshot-install
validation, snapshot-needed and follower-lag diagnostics, cluster backup/restore
dry-run planning, and a published-image cluster smoke checklist.

This release is **not production HA.** It makes no production automatic-failover
claim, and it does not implement linearizable follower reads, distributed
transactions, dynamic membership, sharding, or multi-region — and it claims none
of them. Multi-node mode stays experimental, off by default, and gated behind two
explicit opt-ins. **Single-node mode remains the recommended production mode.**

It changes no on-disk storage format and no wire protocol (AWP 1), and it
preserves Aura Connector 0.3.x compatibility — no connector release is required.
The cluster health report and per-peer status gain additive snapshot/lag
diagnostics fields that older clients ignore.

## At a glance

- **Multi-architecture Docker images.** The release-tag workflow builds and
  pushes a `linux/amd64` + `linux/arm64` manifest to
  `ghcr.io/ohswedd/auradb:0.6.1` and `:latest` via Docker Buildx (the arm64 leg
  is built under QEMU emulation in CI). PR/branch builds build `linux/amd64`
  through buildx without publishing. On Apple Silicon, `docker pull` selects the
  arm64 variant automatically.
- **Larger and concurrent snapshot-install validation.** A CI-safe larger
  snapshot-install run, plus `#[ignore]`d 1,000-entry and 10,000-entry stress
  scenarios, assert that data, secondary indexes, planner statistics, and MVCC
  commit-timestamp order converge after a snapshot install. A concurrent-writes
  scenario keeps the leader committing while a follower installs a snapshot and
  resumes AppendEntries, with no duplicate apply and full convergence.
- **Snapshot-needed and follower-lag diagnostics.** `auradb cluster status
  --addr` now reports, per peer, the replication `lag_entries`, whether the peer
  `needs_snapshot`, whether a snapshot install is in progress, and a
  `catch_up_state` (`normal`, `probing`, `snapshot_needed`,
  `snapshot_installing`, `caught_up`, or `unknown`). It also reports
  cluster-level snapshot diagnostics: the last installed boundary, last install
  time, last rejection reason, bytes sent/installed, and an in-progress gauge. A
  new `auradb cluster doctor --addr` turns these into operator warnings (a
  follower needing a snapshot, a lagging follower, or quorum at the minimum).
- **More snapshot-install metrics.** New gauges/counters:
  `auradb_cluster_snapshot_needed_total`,
  `auradb_cluster_snapshot_bytes_sent_total`,
  `auradb_cluster_snapshot_bytes_installed_total`,
  `auradb_cluster_snapshot_in_progress`, and
  `auradb_cluster_snapshot_last_error`.
- **Cluster backup/restore dry-run tooling.** `auradb cluster backup-plan`
  inspects a data dir and reports what a logical backup would include, exclude,
  where it restores, and which secrets are referenced (redacted, never emitted).
  `auradb cluster restore-plan` inspects a JSONL backup and reports what a
  restore would load and where. Neither command writes any data.
- **Published GHCR cluster smoke checklist.** `scripts/smoke_cluster_compose.sh`
  prints the image used, node ports, leader, quorum, peer states, and the
  teardown result. The release checklist and the manual `published-image-smoke`
  workflow inspect the multi-arch manifest before running the smoke against the
  published image.
- **Connector leader-hint UX review.** Aura Connector 0.3.x stays fully
  compatible but is not cluster-routing-aware; manual leader routing using the
  `not_leader` leader hint is documented, with tests pinning the leader-hint
  message and the no-infinite-retry contract.

## Snapshot install diagnostics (preview)

The v0.6.0 peer snapshot install is unchanged on the wire (a bounded,
single-message transfer). v0.6.1 makes it observable:

- The leader tracks per-peer catch-up state, so an operator can see a follower
  move from `normal` → `snapshot_needed` → `snapshot_installing` → `caught_up`.
- The follower records the last installed boundary (index/term), the last install
  time, and the last rejection reason; rejections (oversized, wrong cluster, bad
  digest, future format, stale term, boundary mismatch) set
  `auradb_cluster_snapshot_last_error` and a status field.
- Bytes sent (leader) and bytes installed (follower) are counted, so transfer
  volume is visible without enabling debug logging.

These are additive: a single-node cluster reports no peers and no snapshot
diagnostics, and older connectors ignore the new fields.

## Cluster backup/restore dry-run

`auradb cluster backup-plan` and `auradb cluster restore-plan` are **planning**
commands — they never write a backup or restore data. They inspect real engine
and cluster metadata (or a real backup file) and report:

- the source mode (`leader-logical-backup` for a cluster node, or
  `local-data-dir-logical-backup`);
- what is included (latest committed state, schema, record/collection counts;
  indexes are rebuilt on restore);
- what is excluded (the Raft log and compaction state, cluster membership / peer
  metadata, uncommitted entries, and historical MVCC versions);
- the restore target (single-node restore into a fresh data dir, optionally
  bootstrapping a fresh single-node preview cluster);
- warnings (you cannot restore directly into a live multi-node cluster; run the
  backup from a stable leader with writes quiesced; verify the backup after
  restore) and a redacted list of referenced secrets (auth token, peer auth
  token, TLS material) that are **not** part of a logical backup.

## Compatibility

- **Storage format:** unchanged (v2).
- **Aura Wire Protocol:** AWP 1, unchanged. The cluster health section and
  per-peer status gain additive snapshot/lag diagnostics fields; the
  `not_leader` error code and the optional `retryable` hint are unchanged.
- **Aura Connector:** 0.3.x remains fully compatible — no connector release is
  required for v0.6.1.
- **Single-node and single-node-cluster behavior:** unchanged.

See [COMPATIBILITY.md](COMPATIBILITY.md) and
[AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md).

## Release validation

In addition to the workspace test suite, fmt/clippy gates, and the artifact
audit, v0.6.1 was validated locally:

- **Docker image** `auradb:0.6.1` builds from the multi-stage `Dockerfile` via
  `docker buildx build --platform linux/amd64 --load`, and
  `docker run --rm auradb:0.6.1 auradb version` prints `auradb 0.6.1`. The
  `linux/arm64` leg of the multi-arch manifest is built by the release-tag
  workflow under QEMU; only `linux/amd64` was built locally. All three Compose
  files (`docker-compose.yml`, `.secure.yml`, `.cluster.yml`) validate.
- **Live Docker Compose cluster** (`AURADB_IMAGE=auradb:0.6.1 bash
  scripts/smoke_cluster_compose.sh`) brings up a three-node mutual-TLS cluster,
  elects a leader, reports quorum and per-peer catch-up state, and tears down
  cleanly.
- **Aura Connector 0.3.x remains wire-compatible.** Local validation used the
  stdlib AWP harness (`tests/conformance/python/run_conformance.py`, 18/18) and
  the Rust conformance crate (`auradb-conformance`). Published Aura Connector
  conformance is covered by CI (`conformance.yml`) and must pass before release —
  no connector release is required.

Post-release, verify the published multi-arch image:

```bash
docker buildx imagetools inspect ghcr.io/ohswedd/auradb:0.6.1
AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.6.1 bash scripts/smoke_cluster_compose.sh
```

## Upgrading

v0.6.1 is a drop-in upgrade from v0.6.0 (and from v0.5.x for single-node and
single-node-cluster deployments): no data migration, no config change, no
connector change. The multi-node preview remains opt-in (`[cluster] enabled =
true` plus `experimental_multi_node = true`) and static-membership only. See
[UPGRADING.md](UPGRADING.md).

## Known limitations

- Multi-node mode is an experimental preview, not production HA. No production
  automatic failover, no linearizable follower reads, no distributed
  transactions, no dynamic membership, no sharding, no multi-region.
- Peer snapshot install remains a bounded single-message transfer (no chunked
  streaming) and targets the strictly-behind follower case.
- `auradb cluster backup-plan` / `restore-plan` are dry-run planners; they do not
  perform the backup or restore. Restoring directly into a live multi-node
  cluster is not supported.
- Local Docker validation built `linux/amd64`; the `linux/arm64` image is built
  by CI under QEMU and verified via `docker buildx imagetools inspect`.
- Single-node mode remains the recommended production mode.
