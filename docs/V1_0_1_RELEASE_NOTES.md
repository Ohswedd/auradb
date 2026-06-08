# AuraDB v1.0.1 release notes

**First production patch — single-node production line, multi-node HA candidate
preview.**

AuraDB v1.0.1 is the first patch on the v1.0 single-node production line. It is a
**documentation, validation, and release-engineering** patch: it re-runs the v1.0
production gates against the v1.0.1 build, refreshes the version pointers and
support documentation, and re-verifies the release-artifact, backup/restore,
upgrade, security, Docker, and connector-conformance gates that back the v1.0
production-support statement.

It carries forward **all** v1.0.0 behavior. It adds **no** new database or cluster
architecture, changes **no** semantics, and touches **no** on-disk or wire format.
**Single-node mode remains the recommended production mode.** Multi-node static
clustering remains an **HA candidate preview** — strong release-candidate evidence,
but **not** a production HA guarantee, no production automatic failover, no
production cluster readiness.

## Compatibility

- **Storage format unchanged** (manifest `format_version` 2). Data directories from
  every prior v0.3.x–v1.0.x release open in place; no migration is required. A v1
  (≤ 0.2.x) directory still migrates to v2 transparently on first open.
- **Aura Wire Protocol unchanged** (AWP 1).
- **Aura Connector v0.4.1 compatible** (and compatible 0.4.x). v1.0.1 is an
  AuraDB-only patch; the connector is unchanged.
- **Configuration is backward compatible.** No new config fields are added; no
  config flag changes meaning.
- All v1.0.0 behavior is preserved except where a documented bug is fixed.

The frozen v1 compatibility surfaces are unchanged: **Aura Wire Protocol 1** is the
stable v1 wire protocol and **storage format v2** is the stable v1 single-node
storage format, each preserved across v1.x unless a security, correctness, safety,
or corruption issue requires a documented change or migration. See
[`COMPATIBILITY.md`](COMPATIBILITY.md), [`PROTOCOL.md`](PROTOCOL.md),
[`STORAGE_ENGINE.md`](STORAGE_ENGINE.md), and
[`AURA_CONNECTOR_COMPATIBILITY.md`](AURA_CONNECTOR_COMPATIBILITY.md).

## What changed in v1.0.1

- **Version bump** to `1.0.1` across the workspace, CLI (`auradb version` /
  `auradb compatibility`), documentation, the cluster Compose example, and the
  benchmark baseline.
- **Re-verified release gates on the v1.0.1 build.** The format, clippy, full test
  suite, backup/restore gate, upgrade gate, `cargo audit` / `cargo deny`,
  release-artifact verifier self-test, Docker image and Compose smokes, and
  connector conformance were re-run against v1.0.1.
- **Refreshed support and production documentation.** Current-release version
  pointers and the compatibility matrix now name v1.0.1; the single-node production
  support statement and the multi-node HA-candidate-preview disclaimer are
  unchanged in substance.
- **Refreshed benchmark baseline** (`benches/baseline/v1.0.1.json`). Benchmark
  numbers remain machine-specific and **warn-only** — never a release gate.
- **Fixed the example Docker Compose image tags.** The developer-quickstart
  (`docker-compose.yml`) and secure-deployment (`docker-compose.secure.yml`)
  examples had remained pinned to the obsolete `0.2.1` image tag; they now
  reference the current `ghcr.io/ohswedd/auradb:1.0.1` image, so the secure example
  no longer pulls a long-superseded image.

## Support policy

The authoritative statement is [`SUPPORT_POLICY.md`](SUPPORT_POLICY.md). v1.0.1
supports production **single-node** deployments configured with authentication,
TLS, scheduled backups with a rehearsed restore, monitoring, and the documented
runbooks. This is the recommended production deployment mode. It does **not** claim
that all deployments are production-ready, and the multi-node cluster is a preview,
not production HA.

## Upgrading

v1.0.1 is a **drop-in** binary replacement over v1.0.0 (and earlier v2-format
releases): no storage migration, no config change. As always, take a backup and run
a restore drill before upgrading, and run `auradb check` before and after; there is
**no** downgrade guarantee and rollback means restore from backup. See
[`UPGRADING.md`](UPGRADING.md) and [`RUNBOOKS.md`](RUNBOOKS.md).

## What this release is not

v1.0.1 does **not** claim production HA, production clustering, production automatic
failover, production cluster readiness, linearizable follower reads, distributed or
cross-shard transactions, dynamic or online membership, sharding, multi-region,
serializable isolation, ANN/HNSW, BM25, hybrid fusion, a Kubernetes operator, or
SDKs beyond the current connector scope. None of those are present. Single-node
mode is the recommended production mode; multi-node remains an HA candidate
preview. See [`SUPPORT_POLICY.md`](SUPPORT_POLICY.md),
[`HA_RELEASE_CANDIDATE.md`](HA_RELEASE_CANDIDATE.md),
[`V1_0_DECISION_CHECKLIST.md`](V1_0_DECISION_CHECKLIST.md), and
[`PRODUCTION_READINESS.md`](PRODUCTION_READINESS.md).
