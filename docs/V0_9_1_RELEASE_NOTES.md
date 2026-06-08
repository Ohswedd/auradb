# AuraDB v0.9.1 release notes

**HA release-candidate stabilization for the v0.9.0 candidate.**

AuraDB v0.9.1 is a narrow stabilization patch over the v0.9.0 HA release
candidate. It polishes leader-hint propagation, strengthens leader-hint
documentation and tests, improves the HA smoke's reliability and diagnostics,
adds snapshot/compaction coverage across a leader change, and clarifies the
operator runbooks. It adds **no** new cluster features and makes **no** new
guarantees beyond what the tests demonstrate.

It is **not** production HA. **Single-node mode remains the recommended
production mode.** Multi-node remains an HA release candidate for the controlled
static-cluster preview — not a production HA guarantee.

## Compatibility

- **Storage format unchanged** (manifest `format_version` 2). Data directories
  from every prior release open in place; v0.9.0 directories need no migration.
- **Aura Wire Protocol unchanged** (AWP 1).
- **Aura Connector v0.4.1 compatible** (and earlier as before). This is an
  AuraDB-only patch; the connector is unchanged.
- **Configuration is backward compatible.** The new `[cluster]`
  `advertise_client_addr` is optional and additive: existing configs that omit it
  behave exactly as in v0.9.0.
- All v0.9.0 behavior is preserved except where a documented bug is fixed.

## Highlights

### Leader `client_addr` self-report (`advertise_client_addr`)

In v0.9.0, a node could name *another* peer's client address in a `not_leader`
hint and in cluster diagnostics (from each peer's declared `client_addr`), but it
could never name **its own** — a node does not appear in its own peer list. So
when you queried the *leader* directly (for example the new leader right after an
election, as the published-image HA smoke does), its self-reported
`leader_client_addr` was empty, and clients fell back to re-resolving the leader.
That fallback was correct, but the hint should have been present.

v0.9.1 adds an optional `[cluster] advertise_client_addr` — this node's own
client-facing address. When set, a node reports it as the leader client address
**while it is the leader**, in both the `not_leader` hint and cluster
status/health. The value is operator-declared and honest: it is never guessed,
never the peer *transport* address, and is omitted (clients re-resolve) when not
declared. It should match the `client_addr` the other nodes list for this node.

The shipped example and Compose cluster configs now declare it.

### Stronger leader-hint tests

New tests pin the propagation contract:

- the leader hint follows the **new** leader after a re-election, with the new
  leader naming its own client address;
- the hint is the declared client address, **never** a peer transport address;
- the hint is omitted safely (honest "unknown") when no client address is
  declared;
- cluster status and the `not_leader` hint name the **same** leader client
  address;
- the shipped Compose configs declare a usable `advertise_client_addr` and peer
  `client_addr` (verified by loading the configs).

### HA smoke reliability and diagnostics

`scripts/smoke_ha_candidate.sh` now prints the old leader, the new leader, and
all candidate addresses; reports the `leader_client_addr` hint at the initial and
new leader; and states clearly that the Compose hint is the *in-Docker-network*
client address, so a **host** client re-resolves the leader by host port — the
documented fallback, not a failure. `run_connector_leader_change.py` now reports
which path resolved the leader (direct `not_leader` hint vs. re-resolve
fallback), preferring the hint and only then probing the candidates.

### Snapshot and compaction across a leader change

New CI-safe tests exercise the v0.9.0 snapshot/compaction edges specifically
*after* a leader change: a snapshot install brings the rejoined old leader
current; the old leader rejoins as a follower and catches up; snapshot
diagnostics stay consistent (installed bytes, install boundary, no apply errors,
a recorded leader change); the new leader can compact its log and keep serving
writes; and a corrupt install delivered after the change is rejected safely and
is idempotent on retry. Heavier variants remain `#[ignore]`d.

### Clearer operator runbooks

The runbooks and cluster-troubleshooting guide now spell out what to do when a
`not_leader` response lacks `leader_client_addr`: how to re-resolve the leader
(`auradb cluster leader`, `auradb cluster status --json`, then try the candidate
client addresses), how to tell a stale hint apart from "no leader", how to
proceed after a leader change, which logs to collect, how to run
`smoke_ha_candidate.sh` and what counts as pass vs. the expected fallback, and
when to restore from backup.

## Upgrading

v0.9.1 is a drop-in patch over v0.9.0. There is no storage migration. To benefit
from the leader self-report, add `advertise_client_addr` to each node's
`[cluster]` section (its own client address, matching what peers list for it);
omitting it preserves v0.9.0 behavior. As always, take a backup and run a restore
drill before upgrading; see [`UPGRADING.md`](UPGRADING.md) and
[`RUNBOOKS.md`](RUNBOOKS.md).

## What this release is not

v0.9.1 does **not** claim production HA, production clustering, production
automatic failover, linearizable follower reads, distributed transactions,
dynamic membership, sharding, multi-region, serializable isolation, ANN/HNSW,
BM25, or hybrid fusion. None of those are present. Single-node mode remains the
recommended production mode; multi-node remains an HA release candidate for the
controlled static-cluster preview. See [`HA_RELEASE_CANDIDATE.md`](HA_RELEASE_CANDIDATE.md)
and [`PRODUCTION_READINESS.md`](PRODUCTION_READINESS.md).
