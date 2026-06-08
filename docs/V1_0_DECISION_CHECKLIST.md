# v1.0 decision checklist

> **This document defines the criteria for the AuraDB v1.0.0 decision and records
> the decision that was made.** AuraDB v1.0.0 ships as a **single-node production
> release with a multi-node HA candidate preview**, exactly the scope this
> checklist's evidence supports. Single-node mode is the recommended production
> mode; multi-node is an HA candidate preview, **not** production HA.

The purpose of this checklist is to make the v1.0 decision **honest and
evidence-driven**: every claim v1.0 makes is backed by the evidence listed here,
and any claim whose evidence is missing is deferred or explicitly scoped out.
v1.0.0 makes the single-node production claim (§1, §3) and scopes production HA out
(§2, §4, §6). It complements the [support policy](SUPPORT_POLICY.md), the strict
production-HA criteria in [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) §8, and
the posture in [PRODUCTION_READINESS.md](PRODUCTION_READINESS.md).

## 1. What v1.0 claims

Based on the evidence in §5, v1.0.0 **honestly claims**:

- **Single-node production support.** Commits go straight to durable local
  storage. This is the default, the recommended production mode, and the most
  thoroughly tested path: MVCC single-node snapshot isolation, a cost-based query
  planner, persisted indexes (primary, unique, secondary, document-path,
  full-text term-frequency, exact vector), durable recovery and corruption
  handling, `auradb check` / `doctor` consistency reporting, enforced
  Argon2id token auth and server-terminated TLS, logical backup/restore with
  `backup verify`, configurable `[limits]`, a published multi-arch Docker image,
  and release-artifact reproducibility.
- **A controlled static-cluster HA *preview*** (not production HA): real
  cross-process Raft leader election, majority-commit replication, follower
  catch-up, a bounded single-message snapshot install, fail-stop recovery, and
  connector leader-redirect — validated against the failure matrix in
  [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) §3, at preview scale, on
  loopback.

## 2. What v1.0 does not claim

v1.0.0 **does not claim** (the evidence does not exist):

- production HA or production cluster readiness;
- production automatic failover or any recovery-time / recovery-point objective;
- linearizable reads or follower reads (followers reject reads by default);
- distributed or cross-shard transactions;
- dynamic cluster membership (`join` / `leave` / `step-down`, joint consensus);
- sharding or multi-region deployment;
- serializable isolation (single-node isolation is snapshot isolation);
- ANN / HNSW vector search, BM25 ranking, or hybrid fusion (exact vector search
  and term-frequency full-text are what ship).

## 3. Required for single-node production support

The single-node path is the v1.0 production-supported mode. Required and **met**:

| Requirement | Status | Evidence |
| ----------- | ------ | -------- |
| Durable storage + crash recovery | Met | recovery/corruption fuzz tests; `auradb check --json` drills |
| MVCC snapshot isolation + GC | Met | transaction/MVCC tests; GC validation |
| Enforced auth (Argon2id) + TLS | Met | auth/TLS conformance ([SECURITY.md](SECURITY.md)) |
| Persisted indexes + safe rebuild | Met | index snapshot/fingerprint tests |
| Backup / restore + `backup verify` | Met | backup/restore drills over genuine fixtures |
| Upgrade safety across versions | Met | upgrade-fixture tests ([UPGRADING.md](UPGRADING.md)) |
| Resource limits | Met | `[limits]` enforcement tests |
| Published image + signed artifacts | Met | Docker workflow; `verify_release_artifacts.sh` |
| Operator runbooks + observability | Met | [RUNBOOKS.md](RUNBOOKS.md), [OBSERVABILITY.md](OBSERVABILITY.md) |
| Longer single-node soak with documented results | Recommended | `scripts/soak_single_node.sh` ships; operators should run a multi-hour/day soak for their workload and hardware. The core durability, recovery, MVCC, backup, upgrade, and security requirements above are met independently of soak duration; a longer soak is additional operational validation, not a gate on the scoped single-node claim |

## 4. Required for production HA support

Production HA is **not** a v1.0 candidate unless **all** of
[HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) §8 are met and documented with
evidence. Summarized:

| Requirement | Status |
| ----------- | ------ |
| Repeated long (multi-hour/day) fail-stop + chaos soak, zero data loss / duplicate apply | Missing (CI-safe short cycles only) |
| Snapshot install under large/production state (chunked/streaming if needed) | Missing (single bounded 8 MiB message only) |
| Backup/restore **cluster** drills with documented RPO/RTO | Missing (single-node restore path only) |
| Partition/heal across real networks (namespaces, container nets, cross-host) | Missing (loopback simulation only) |
| Disk-full and I/O-error behavior, defined and tested | Missing (disk-pressure warning only) |
| Process-supervisor (systemd / Docker / Kubernetes) integration documented | Partial (Docker restart policy; no k8s) |
| TLS and token rotation drills without downtime | Partial (manual documented drills) |
| Published SLOs and explicit non-goals | Missing (non-goals documented; no SLOs) |
| Connector leader-change behavior across every supported client | Partial (Python connector validated) |
| Operational monitoring / alert thresholds documented | Partial (metrics + doctor warnings; no alert thresholds) |
| External feedback / dogfood period | Missing |

Until every row is **Met** and documented, multi-node remains a controlled
static-cluster preview.

## 5. Evidence that exists

- **Tests.** The full `cargo test --workspace --all-features` suite, plus the
  serial multi-node cluster suite (`cargo test -p auradb-replication --test
  multi_node -- --test-threads=1`): leader election, replicated writes, follower
  catch-up, repeated fail-stop cycles, snapshot install after compaction, snapshot
  install across a leader change, old-leader rejoin, no-duplicate-apply,
  partition/heal, reconnect storm, peer TLS / token rejection, and the
  leader-hint / `advertise_client_addr` contract. See [TESTING.md](TESTING.md) and
  the failure matrix and coverage map in
  [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md).
- **Smokes.** `scripts/smoke_cluster_compose.sh` (Compose cluster smoke) and
  `scripts/smoke_ha_candidate.sh` (end-to-end HA candidate smoke: leader kill,
  re-election, write through new leader, old-leader rejoin, connector
  leader-change), both with image digest, per-node versions, leader before/after,
  leader client-address source, and explicit pass/fail criteria.
- **Docker published-image validation.** Multi-arch (`linux/amd64` +
  `linux/arm64`) build and `auradb version` check; Compose config validation for
  `docker-compose.yml`, `docker-compose.secure.yml`, and
  `docker-compose.cluster.yml`.
- **Connector conformance.** `run_connector_smoke.py`,
  `run_connector_conformance.py`, `run_connector_cluster.py`, and
  `run_connector_leader_change.py` against Aura Connector v0.4.1. See
  [CONFORMANCE.md](CONFORMANCE.md) and
  [AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md).
- **Backup/restore drills.** Logical backup → `backup verify` → restore into a
  fresh single-node directory, validated around a leader change.
- **Upgrade drills.** Upgrade-safety tests across genuine prior-version fixtures.
- **Release artifacts.** `scripts/verify_release_artifacts.sh`, `SHA256SUMS`, and
  the release workflow. See [RELEASE.md](RELEASE.md).

## 6. Evidence still missing

- **Cross-host / real-network chaos.** Partition/heal beyond loopback: network
  namespaces, container networks, and genuine cross-host clusters.
- **Longer soak.** Multi-hour/day repeated fail-stop and chaos soak with documented
  zero-data-loss / zero-duplicate-apply results (the scripts exist; a documented
  run does not).
- **Disk-full / I/O-error drills.** Defined, tested behavior under a full disk and
  injected I/O errors, with operator guidance.
- **Snapshot streaming / large-state alternative.** The current install is a single
  bounded 8 MiB message; production-size state needs chunked/streaming transfer or
  a documented size bound.
- **Operator SLOs.** Published recovery-time / recovery-point objectives and alert
  thresholds.
- **External dogfood period.** A sustained external-user or dogfood validation
  window, with the issues it surfaces addressed.

## 7. v1.0 scope (decided)

- **A single-node production-support release.** The single-node path has the
  evidence (§3); a longer single-node soak is recommended operational validation,
  not a gate on the scoped claim.
- **HA remains a preview.** The missing evidence in §6 (and
  [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) §8) is not yet closed, so
  multi-node ships as an HA candidate preview, not production HA.

## 8. Required language for v1.0 (HA is not production)

v1.0 ships before the production-HA evidence is complete, so the release states,
prominently and consistently across README, release notes, and docs:

- **AuraDB v1.0 is a single-node production release.**
- **Multi-node HA remains a controlled static-cluster preview — not production
  HA.** It must not claim production HA, production automatic failover, production
  cluster readiness, linearizable/follower reads, distributed transactions,
  dynamic membership, sharding, or multi-region.
- **Single-node mode remains the recommended production mode.**

See [ROADMAP.md](ROADMAP.md), [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md),
and [PRODUCTION_READINESS.md](PRODUCTION_READINESS.md).
