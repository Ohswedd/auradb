# AuraDB v0.9.2 release notes

**Final HA candidate stabilization.**

AuraDB v0.9.2 is the **last planned stabilization patch** for the HA release
candidate before deciding what AuraDB v1.0.0 can honestly claim. It finalizes the
HA candidate evidence and gap list, adds a v1.0 decision checklist, strengthens
the leader-hint / client-address tests and runbooks after `advertise_client_addr`,
sharpens the HA smoke diagnostics and the published-image post-release checklist,
and maps the snapshot/compaction/old-leader-rejoin coverage so it is auditable
without duplicate tests. It adds **no** new cluster features and makes **no** new
guarantees beyond what the tests demonstrate.

It is **not** production HA. **Single-node mode remains the recommended production
mode.** Multi-node remains an HA release candidate for the controlled
static-cluster preview — not a production HA guarantee.

## v0.9.1 aftermath

v0.9.2 began with a full review of the v0.9.1 release, CI, smoke, and docs for
GHCR publish issues, HA-smoke flakiness, leader-hint / `advertise_client_addr`
issues, Docker in-network vs. host-published address confusion, connector
conformance issues, workflow warnings, flaky serial cluster tests, and
documentation overclaims. **No code-behavior regression was found in v0.9.1.**
v0.9.2 is therefore a **proactive final stabilization patch**: it consolidates
evidence, sharpens diagnostics and operator guidance, and adds the v1.0 decision
criteria — without changing v0.9.1 behavior.

## Compatibility

- **Storage format unchanged** (manifest `format_version` 2). Data directories
  from every prior release open in place; no migration is required.
- **Aura Wire Protocol unchanged** (AWP 1).
- **Aura Connector v0.4.1 compatible** (and earlier as before). This is an
  AuraDB-only patch; the connector is unchanged.
- **Configuration is backward compatible.** No new config fields are added;
  `[cluster] advertise_client_addr` (v0.9.1) remains optional and additive.
- All v0.9.1 behavior is preserved except where a documented bug is fixed.

## Highlights

### v1.0 decision checklist

[`docs/V1_0_DECISION_CHECKLIST.md`](V1_0_DECISION_CHECKLIST.md) is the single
honest answer to "what can v1.0 claim?" It records what v1.0 **can** claim today
(single-node production support is the candidate path), what it **cannot** claim
(production HA, production automatic failover, linearizable follower reads,
distributed transactions, dynamic membership, sharding, multi-region), the
requirements for single-node production support and for production HA, the
evidence that exists today (tests, smokes, Docker published-image validation,
connector conformance, backup/restore and upgrade drills, release artifacts), the
evidence still missing (cross-host chaos, longer soak, disk-full / I/O-error
drills, snapshot streaming for large state, operator SLOs, an external dogfood
period), and the recommended v1.0 scope. It does **not** pre-decide v1.0 — it
defines the criteria. None of the production-HA criteria are expected to be met in
v0.9.2.

### Stronger leader-hint / client-address tests

Building on the v0.9.1 `advertise_client_addr` self-report, v0.9.2 adds tests that
pin the contract across **multiple** leader changes and an old-leader rejoin:

- `not_leader_uses_advertised_client_addr_after_multiple_re_elections` — after two
  kill/elect/rejoin cycles, whichever node currently leads reports its **own**
  declared client address as the hint, and a follower converges on that same
  address.
- `not_leader_hint_survives_old_leader_rejoin` — when a stopped old leader rejoins
  as a follower, the leader hint names the **current** leader's client address
  (never the rejoined node's stale one), from both the rejoined node and a
  survivor.
- `docker_compose_docs_explain_in_network_vs_host_client_addr` — a
  docs-consistency test asserting the operator docs explain the in-Docker-network
  vs. host-published client-address distinction (so the documented host re-resolve
  fallback is not mistaken for a failure).

These complement, and do not duplicate, the v0.9.1 leader-hint tests
(`not_leader_includes_leader_client_addr_after_re_election`,
`not_leader_hint_does_not_use_peer_addr_as_client_addr`,
`not_leader_hint_omits_unknown_client_addr_safely`,
`cluster_status_leader_client_addr_matches_not_leader_hint`,
`leader_reports_its_own_client_addr_in_health`,
`docker_compose_cluster_not_leader_hint_has_client_addr_if_configured`).

### Snapshot / compaction / old-leader-rejoin coverage mapping

Rather than add duplicate snapshot tests, v0.9.2 **maps** the final
snapshot/compaction/rejoin scenarios to the existing v0.9.x tests that already
cover them (see [`HA_RELEASE_CANDIDATE.md`](HA_RELEASE_CANDIDATE.md) §10 and
[`TESTING.md`](TESTING.md)): old-leader rejoin after compaction
(`snapshot_install_after_leader_change`,
`old_leader_rejoins_then_receives_snapshot_if_needed`), no duplicate apply across
repeated cycles (`ha_repeated_restart_no_duplicate_apply`,
`ha_old_leader_rejoins_each_cycle`), snapshot metrics consistency across leader
changes (`snapshot_metrics_after_leader_change`, `ha_snapshot_metrics_after_install`),
compaction-boundary safety after a leader change (`compaction_after_leader_change`),
snapshot-install failure retry after a leader change
(`snapshot_failure_after_leader_change_safe_to_retry`,
`ha_snapshot_failure_safe_to_retry`), and indexed-workload validity after catch-up
(`ha_snapshot_install_preserves_indexed_workload`, `snapshot_install_preserves_indexes`).

### HA smoke and published-image post-release checklist

`scripts/smoke_ha_candidate.sh` and `scripts/smoke_cluster_compose.sh` print the
image digest when available, the server version from each node, the leader before
and after the kill, the leader client-address **source** (advertised / status /
fallback / probe), the connector version, and the exact pass/fail criteria; they
preserve logs on failure, tear down cleanly on success, and honor
`KEEP_ARTIFACTS=1`. Both are documented as **post-release gates** (manual /
post-release in CI), **not** PR blockers. See [`RELEASE.md`](RELEASE.md) and
[`TESTING.md`](TESTING.md).

### Clearer operator runbooks

The runbooks and cluster-troubleshooting guide add a final pass on: a missing
leader hint, an unreachable leader hint, Docker in-network vs. host-published
addresses, when and how to set and rotate `advertise_client_addr`, telling a
routing issue apart from a no-leader issue, running both published-image smokes,
collecting evidence for a v1.0 readiness report, why restore is for data recovery
(not routing), and when to stay on single-node production mode.

## Upgrading

v0.9.2 is a drop-in patch over v0.9.1. There is no storage migration and no config
change. As always, take a backup and run a restore drill before upgrading; see
[`UPGRADING.md`](UPGRADING.md) and [`RUNBOOKS.md`](RUNBOOKS.md).

## What this release is not

v0.9.2 does **not** claim production HA, production clustering, production
automatic failover, production cluster readiness, linearizable follower reads,
distributed transactions, dynamic membership, sharding, multi-region, serializable
isolation, ANN/HNSW, BM25, or hybrid fusion. None of those are present. Single-node
mode remains the recommended production mode; multi-node remains an HA release
candidate for the controlled static-cluster preview. See
[`HA_RELEASE_CANDIDATE.md`](HA_RELEASE_CANDIDATE.md),
[`V1_0_DECISION_CHECKLIST.md`](V1_0_DECISION_CHECKLIST.md), and
[`PRODUCTION_READINESS.md`](PRODUCTION_READINESS.md).
