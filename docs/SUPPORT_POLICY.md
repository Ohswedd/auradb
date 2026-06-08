# AuraDB v1.0 support policy

> **AuraDB v1.0.0 supports production single-node deployments when configured
> with auth, TLS, backups, monitoring, and the documented runbooks. Multi-node
> static clustering remains an HA candidate preview, not production HA.**

This document is the authoritative statement of what AuraDB v1.0 supports, at what
level, and for how long. It is written to be precise rather than expansive: a
capability appears under **Supported** only when validation backs it. Anything
whose evidence is incomplete is named under **Preview** or **Not supported** so
the boundary is unambiguous.

It complements the production checklists in
[PRODUCTION_READINESS.md](PRODUCTION_READINESS.md), the v1.0 scope record in
[V1_0_DECISION_CHECKLIST.md](V1_0_DECISION_CHECKLIST.md), the strict production-HA
criteria in [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md), and the
compatibility matrix in [COMPATIBILITY.md](COMPATIBILITY.md).

## Production support statement

AuraDB v1.0.0 supports production **single-node** deployments when configured with
authentication, TLS, scheduled backups with a rehearsed restore, monitoring, and
the documented runbooks. This is the recommended production deployment mode.

This is a **scoped** statement, not a blanket one. AuraDB does **not** claim that
all deployments are production-ready: a single-node deployment that omits auth,
TLS, backups, or monitoring is not a supported production configuration, and the
multi-node cluster is a preview, not production HA.

## Supported in v1.0

These are validated and supported for production single-node use, run per the
[single-node production runbook](PRODUCTION_READINESS.md):

- **Single-node AuraDB** — durable local storage, crash recovery, MVCC single-node
  snapshot isolation, version GC, and a cost-based query planner.
- **Authentication and TLS for network exposure** — enforced static-token auth
  (Argon2id) and server-terminated TLS with optional mutual TLS (rustls), both
  fail-closed. See [SECURITY.md](SECURITY.md).
- **Backup and restore** — logical dump (`auradb dump`), non-importing validation
  (`auradb backup verify`), and restore (`auradb restore`) into a fresh data
  directory, with `auradb check` consistency reporting.
- **Upgrade** from all documented supported release fixtures (see
  [UPGRADING.md](UPGRADING.md) and `tests/fixtures/`).
- **Aura Connector v0.4.1** (and compatible 0.4.x).
- **Aura Wire Protocol 1 (AWP 1)** compatibility within v1.x, unless a security or
  correctness issue requires a documented change.
- **Storage format v2** compatibility within v1.x, unless a safety, corruption, or
  security issue requires a documented migration.
- **Docker image and release binaries** — the published multi-arch
  (`linux/amd64` + `linux/arm64`) GHCR image and the five prebuilt binary archives
  with `SHA256SUMS`.
- **CLI administration tools** — `init`, `check`, `doctor`, `compact`, `gc`,
  `stats`, `dump`/`restore`/`backup verify`, `index check`/`rebuild`,
  `auth hash-token`/`rotate-token`, `cert generate-dev`, and the rest of the
  documented surface ([CLI.md](CLI.md)).
- **Observability and health endpoints** — JSON and Prometheus metrics, structured
  tracing, and `auradb status`/`doctor --json` ([OBSERVABILITY.md](OBSERVABILITY.md)).
- **Exact vector search** (`cosine`, `euclidean`, `dot_product`) and **tokenized
  full-text search** with term-frequency ranking.

## Preview in v1.0 (not production HA)

These work, are tested at preview scale, and are useful for evaluation — but they
are **not** production-supported and must not carry production traffic that
requires high availability:

- **Multi-node static cluster.** Real cross-process Raft leader election,
  majority-commit replication, and follower catch-up over an authenticated peer
  transport, off by default and gated by two explicit `[cluster]` opt-ins. Static
  membership.
- **HA candidate preview.** Multi-node static clustering in v1.0 remains an HA
  candidate preview. It has strong release-candidate evidence, but it is not a
  production HA guarantee.
- **Raft replication preview** — the replicated command path and bounded
  single-message snapshot install (capped at 8 MiB).
- **Peer networking** — the frame-checked, authenticated peer transport; off by
  default, loopback unless TLS and a peer token are configured for a non-loopback
  bind.
- **Cluster fail-stop recovery smokes** — leader kill / re-election / old-leader
  rejoin, validated by the HA candidate smoke and CI fail-stop cycles.
- **Connector leader-redirect ergonomics** — the additive structured `not_leader`
  object and Aura Connector 0.4.x redirect helpers.

## Not supported in v1.0

AuraDB v1.0 does **not** provide, and **must not** be relied on for, any of:

- production HA guarantee;
- automatic failover SLA;
- dynamic cluster membership;
- online membership changes (`join` / `leave` / `step-down`, joint consensus);
- linearizable follower reads (followers reject client reads);
- distributed transactions;
- cross-shard transactions;
- sharding;
- multi-region deployment;
- serializable isolation (single-node isolation is snapshot isolation);
- approximate nearest-neighbour vector search (ANN / HNSW);
- BM25 ranking or hybrid lexical-vector fusion;
- a Kubernetes operator;
- a managed cloud service;
- official SDKs beyond the current Aura Connector scope.

## Security support

- **Reporting vulnerabilities.** Report security issues privately as described in
  [SECURITY.md](SECURITY.md) and the repository's [SECURITY.md](../SECURITY.md)
  policy. Do not open public issues for suspected vulnerabilities.
- **Supported versions.** Security fixes target the current v1.x line. The most
  recent v1.x release is the supported baseline; older v0.x releases are not
  maintained for security backports.
- **Backport policy.** Security fixes are delivered in a new patch release on the
  current v1.x line. There is no commitment to backport to v0.x.
- **Dependency advisory policy.** CI runs `cargo audit` and `cargo deny` (see
  `.github/workflows/security.yml`). A new advisory in the dependency tree is
  triaged before release.
- **Accepted advisory policy.** When an advisory cannot be resolved by upgrading
  (for example, an unmaintained transitive crate with no safe drop-in), it is
  documented with an explicit rationale in `deny.toml` and reviewed each release;
  an *accepted* advisory must be a maintenance/informational notice, not an
  exploitable vulnerability in AuraDB's usage.

## Upgrade support

- **Supported upgrade paths.** In-place upgrade from documented v0.x release
  formats covered by genuine or representative release fixtures (see
  [UPGRADING.md](UPGRADING.md)). Storage format v2 directories (AuraDB ≥ 0.3.0)
  open directly; v1 directories (AuraDB ≤ 0.2.x) migrate to v2 transparently on
  first open. An unknown future storage format is rejected, never silently
  downgraded.
- **Backup-first requirement.** Take a backup (`auradb dump` +
  `auradb backup verify`) before upgrading any deployment, and run
  `auradb check` before and after the upgrade.
- **No downgrade guarantee.** AuraDB does not guarantee that a newer release's data
  directory can be reopened by an older binary, unless a specific downgrade path is
  documented in [UPGRADING.md](UPGRADING.md).
- **Restore-from-backup rollback path.** Rollback means restoring the pre-upgrade
  backup into a fresh data directory. Keep the previous binary and the pre-upgrade
  backup until the upgrade is verified in production.

## Changes to this policy

This policy applies to AuraDB v1.0.0 and the v1.x line. Material changes — for
example promoting multi-node from preview to production HA once the evidence in
[HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) §8 is complete — will be
documented here and in the release notes, never claimed implicitly.
