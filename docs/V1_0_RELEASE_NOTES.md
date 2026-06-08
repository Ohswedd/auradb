# AuraDB v1.0.0 release notes

**Single-node production release, multi-node HA candidate preview.**

AuraDB v1.0.0 is the first release to make a production-support statement. It
supports production **single-node** deployments configured with authentication,
TLS, backups, monitoring, and the documented runbooks. It finalizes the v1.0
support policy, freezes the v1 wire and storage compatibility surfaces, states the
upgrade guarantee, and locks in the backup/restore release gate and security
review that back those claims.

It is **not** production HA. **Single-node mode is the recommended production
mode.** Multi-node static clustering remains an **HA candidate preview** — it has
strong release-candidate evidence, but it is not a production HA guarantee.

v1.0.0 carries forward all v0.9.2 behavior. It adds **no** new database or cluster
architecture and changes **no** semantics; it finalizes documentation, policy, and
release engineering on top of the stabilized v0.9.x HA candidate.

## Compatibility

- **Storage format unchanged** (manifest `format_version` 2). Data directories
  from every prior v0.3.x–v0.9.x release open in place; no migration is required.
  A v1 (≤ 0.2.x) directory still migrates to v2 transparently on first open.
- **Aura Wire Protocol unchanged** (AWP 1).
- **Aura Connector v0.4.1 compatible** (and compatible 0.4.x). v1.0.0 is an
  AuraDB-only release; the connector is unchanged.
- **Configuration is backward compatible.** No new config fields are added; no
  config flag changes meaning.
- All v0.9.2 behavior is preserved except where a documented bug is fixed.

## What v1.0 supports

The authoritative statement is [`docs/SUPPORT_POLICY.md`](SUPPORT_POLICY.md).
In summary, v1.0 **supports** for production single-node use: durable single-node
storage with MVCC snapshot isolation and a cost-based planner; enforced
Argon2id auth and rustls TLS; backup/restore with `backup verify`; upgrade from
documented v0.x fixtures; Aura Connector v0.4.1; AWP 1; storage format v2; the
multi-arch Docker image and prebuilt binaries; the CLI administration tools;
observability and health endpoints; exact vector search; and tokenized full-text
search.

## Frozen compatibility surfaces

### Aura Wire Protocol 1

AuraDB v1.0.0 uses Aura Wire Protocol 1. AWP 1 is the stable v1 wire protocol.
AuraDB v1.x will preserve AWP 1 compatibility unless a security or correctness
issue requires a documented compatibility break. The recommended client is Aura
Connector v0.4.1 (and compatible 0.4.x). See [`PROTOCOL.md`](PROTOCOL.md),
[`COMPATIBILITY.md`](COMPATIBILITY.md), and
[`AURA_CONNECTOR_COMPATIBILITY.md`](AURA_CONNECTOR_COMPATIBILITY.md).

### Storage format v2

AuraDB v1.0.0 uses storage format v2. Storage format v2 is the stable v1
single-node storage format. AuraDB v1.x will preserve storage format v2
compatibility unless a safety, corruption, or security issue requires a documented
migration. See [`STORAGE_ENGINE.md`](STORAGE_ENGINE.md) and
[`UPGRADING.md`](UPGRADING.md).

## Upgrade guarantee

AuraDB v1.0.0 supports in-place upgrade from documented v0.x release formats
covered by genuine or representative release fixtures. Operators must take a backup
first and run `auradb check` before and after upgrade. There is **no** downgrade
guarantee; rollback means restore from backup. Some v0.x releases share storage
format v2 and are covered by representative fixtures (the v0.3.0 fixture is the
representative v2 storage fixture for the v0.3.x–v0.9.x range); the coverage is not
overstated. See [`UPGRADING.md`](UPGRADING.md) and
[`SUPPORT_POLICY.md`](SUPPORT_POLICY.md).

## Backup and restore release gate

A v1.0 release must pass a backup/restore gate over a mixed dataset: dump → `backup
verify` → restore into a fresh single-node directory → `auradb check`, followed by
query, relationship-include, vector, full-text, and document-path smokes and
index/stats validation, with no secrets in the verification output. This is
exercised by the backup/restore and upgrade-gate tests in `auradb-cli` and
documented in [`RELEASE.md`](RELEASE.md) and
[`PRODUCTION_READINESS.md`](PRODUCTION_READINESS.md).

## Security review

The v1.0 security posture is reviewed and documented in [`SECURITY.md`](SECURITY.md)
and [`SUPPORT_POLICY.md`](SUPPORT_POLICY.md): auth and TLS required for network
exposure (a public bind without auth is refused unless an explicit dev override is
set); token, peer-token, and certificate rotation procedures; redaction of secrets
from `doctor` / `status` / `config` / `check` / `backup verify` output; a non-root
Docker image and a secure Compose example; and a `cargo audit` / `cargo deny`
policy with documented, reviewed rationale for any accepted advisory.

## Multi-node: HA candidate preview

Multi-node static clustering in v1.0 remains an HA candidate preview. It has strong
release-candidate evidence (leader election, majority-commit replication, follower
catch-up, fail-stop recovery, bounded snapshot install, connector leader-redirect,
all validated against the failure matrix at preview scale on loopback), but it is
**not** a production HA guarantee. The evidence still required before AuraDB ever
claims production HA — cross-host chaos, longer soak, disk-full and I/O-error
drills, larger-state snapshot streaming or an equivalent, documented operator SLOs,
and an external dogfood period — is tracked in
[`HA_RELEASE_CANDIDATE.md`](HA_RELEASE_CANDIDATE.md) §8 and
[`V1_0_DECISION_CHECKLIST.md`](V1_0_DECISION_CHECKLIST.md).

## Release engineering

- **GitHub Actions maintenance.** The Docker publish workflow's actions are kept on
  Node-24-compatible majors; see [`RELEASE.md`](RELEASE.md) for the current state
  and any documented mitigation. The Docker publish security posture (permissions,
  attestations, manifest checks) is unchanged.
- **Tighter release-artifact verification.** `scripts/verify_release_artifacts.sh`
  verifies all five binary archives, `SHA256SUMS` completeness, version-stamped
  archive names, the host-matching binary printing `auradb 1.0.0`, and — in `--tag`
  mode — that the GitHub release body carries the single-node production statement,
  the multi-node preview disclaimer, the AWP 1 and storage v2 statements, and the
  known limitations. It retains a network-free `--self-test`.

## Upgrading

v1.0.0 is a drop-in upgrade over v0.9.2 (and earlier v2-format releases): no
storage migration, no config change. As always, take a backup and run a restore
drill before upgrading, and run `auradb check` before and after; see
[`UPGRADING.md`](UPGRADING.md) and [`RUNBOOKS.md`](RUNBOOKS.md).

## What this release is not

v1.0.0 does **not** claim production HA, production clustering, production automatic
failover, production cluster readiness, linearizable follower reads, distributed or
cross-shard transactions, dynamic or online membership, sharding, multi-region,
serializable isolation, ANN/HNSW, BM25, hybrid fusion, a Kubernetes operator, or
SDKs beyond the current connector scope. None of those are present. Single-node
mode is the recommended production mode; multi-node remains an HA candidate
preview. See [`SUPPORT_POLICY.md`](SUPPORT_POLICY.md),
[`HA_RELEASE_CANDIDATE.md`](HA_RELEASE_CANDIDATE.md),
[`V1_0_DECISION_CHECKLIST.md`](V1_0_DECISION_CHECKLIST.md), and
[`PRODUCTION_READINESS.md`](PRODUCTION_READINESS.md).
