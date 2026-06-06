# AuraDB v0.6.2 release notes

**Theme: Repeated chaos and larger-state recovery hardening.**

AuraDB v0.6.2 hardens repeated chaos and larger-state recovery behavior in the
controlled multi-node **preview**. It is **not production HA. Single-node mode
remains the recommended production mode.**

This is a patch release. It makes the controlled multi-node preview more reliable
under repeated failures, larger data sets, and recovery-heavy scenarios. It adds
repeated leader restart / re-election cycles, larger multi-model data-set
recovery validation, multi-model snapshot install, peer reconnect storm testing,
deterministic network-interruption (partition/heal) simulations, and
recovery-focused diagnostics.

This release makes no production automatic-failover claim, and it does not
implement linearizable follower reads, distributed transactions, dynamic
membership, sharding, or multi-region — and it claims none of them. Multi-node
mode stays experimental, off by default, and gated behind two explicit opt-ins.

It changes no on-disk storage format and no wire protocol (AWP 1), and it
preserves Aura Connector 0.3.x compatibility — **no connector release is
required.** The cluster health report gains one additive `leader_changes` field
that older clients ignore.

## At a glance

- **Repeated leader restart and re-election.** A required two-cycle test kills
  the current leader, lets the surviving majority elect a new one, commits
  through it, restarts the old leader, and repeats — then asserts every node
  reconverges on the identical committed record set with no duplicate apply and
  an incrementing leader-change metric. An `#[ignore]`d five-cycle variant runs
  the same scenario as a stress test.
- **Larger multi-model data-set recovery.** A follower is stopped while the
  majority commits a larger run of records spanning scalar, secondary-indexed,
  full-text, document-path, and vector fields. After it restarts and catches up,
  its record count, spot reads, full-text search, document-path queries, vector
  nearest-neighbor results, and planner-used indexes are checked to match a node
  that never went down. A full-cluster restart re-verifies the converged state.
  An `#[ignore]`d 5,000-record variant runs the same path at stress size.
- **Multi-model snapshot install.** The live majority compacts its log past the
  entries a stopped follower needs, so the follower can only be brought current
  by a snapshot install. After the install, full-text, document-path, and vector
  records are verified intact and consistent with the rest of the cluster.
- **Peer reconnect storm.** A follower is disconnected and reconnected repeatedly
  while the majority keeps committing. Replication recovers each time, there is
  no duplicate apply, and the follower holds a live peer connection after the
  storm. Outbound reconnect backoff stays bounded (as in v0.5.1).
- **Network-interruption (partition/heal) simulations.** A new in-process
  transport partition control drops a peer's inbound frames to simulate a cut
  link without tearing the node down — so the partition heals with the node's
  in-memory Raft state preserved. The tests cover: a majority partition that
  keeps committing, a leader partitioned into a minority that cannot commit, a
  healed partition that repairs a follower's log, and a partitioned leader that
  triggers a re-election and reconverges on heal.
- **Recovery diagnostics.** `auradb cluster status --addr` now reports
  `leader_changes`, a cumulative count that climbs when leadership flaps.
  `auradb cluster doctor --addr` adds two warnings: a peer **reconnect storm**
  (a peer still disconnected after many connection attempts) and **repeated
  leader changes** (leadership instability).
- **Published-image smoke retained as a release gate.** The release checklist
  still requires waiting for and inspecting the GHCR multi-arch manifest and
  running the published-image compose smoke (leader election, quorum, peer
  states caught up, clean teardown) before a release is considered done.

## What this release is not

AuraDB v0.6.2 is a controlled, opt-in, static-membership **preview** of
multi-node replication. It is explicitly **not**:

- production high availability or production automatic failover;
- linearizable (or otherwise strongly consistent) follower reads — followers are
  not read replicas in this preview;
- distributed or cross-node transactions;
- dynamic cluster membership — there are no join/leave/step-down commands;
- sharding, multi-region, or any horizontal partitioning of data.

It also does not change AuraDB's single-node guarantees: optimistic
read-your-writes transactions (not serializable), exact vector search (no
ANN/HNSW), and tokenized boolean full-text (no BM25 or hybrid fusion).

**Single-node mode remains the recommended production mode.**

## Preview guardrails (unchanged)

Multi-node mode requires both opt-ins in `[cluster]`:

```toml
[cluster]
enabled = true
experimental_multi_node = true
```

Membership is static (peers are listed in config; there are no membership
commands). Non-loopback cluster networking requires TLS **and** a peer auth
token; an unauthenticated public peer bind is refused. The published-image Docker
cluster is a preview workflow. Connector write recovery against a leadership
change remains explicit and bounded (see
[AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md)).

## Upgrading

v0.6.2 is a drop-in upgrade from v0.6.x. There is no storage-format or wire
change, and no configuration change is required. See
[UPGRADING.md](UPGRADING.md).

## Diagnostics quick reference

```bash
# Live cluster health, now including leader_changes.
auradb cluster status --addr 127.0.0.1:7101

# Operator warnings, now including reconnect-storm and repeated-leader-change.
auradb cluster doctor --addr 127.0.0.1:7101
```

## Verification

```bash
cargo test --workspace --all-features
# Heavy/stress recovery scenarios are #[ignore]d by default:
cargo test -p auradb-replication --test multi_node -- --ignored --test-threads=1
```

Post-release published-image smoke:

```bash
AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.6.2 bash scripts/smoke_cluster_compose.sh
```
