# Clustering

> **AuraDB v1.0.1 is a single-node production release; multi-node static
> clustering remains an HA candidate preview, not a production HA guarantee.
> Single-node mode remains the recommended production mode.** Multi-node static
> clustering in v1.0 remains an HA candidate preview. It has strong
> release-candidate evidence, but it is not a production HA guarantee. v1.0.1 adds
> no new cluster architecture and changes no Raft, storage (v2), or wire (AWP 1)
> semantics over v1.0.0; Aura Connector v0.4.1 stays compatible. The cluster
> backup story remains a **leader logical export → single-node restore** path. The
> evidence still outstanding before any production HA claim includes cross-host
> chaos, longer soak, disk-full and I/O-error drills, larger-state snapshot
> streaming (the current install is a single bounded 8 MiB message), documented
> operator SLOs, and an external dogfood period. See
> [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) and
> [V0_9_RELEASE_NOTES.md](V0_9_RELEASE_NOTES.md).

> **AuraDB v0.9.0 is an HA release candidate for the controlled static-cluster
> preview, not a production HA guarantee. Single-node mode remains the
> recommended production mode.** v0.9.0 strengthens failure testing, cluster
> diagnostics, snapshot/compaction coverage, connector behavior under leader
> change, operator recovery runbooks, and the cluster backup/restore story. It
> adds no new cluster architecture and changes no Raft, storage (v2), or wire
> (AWP 1) semantics. The cluster backup story remains a **leader logical export →
> single-node restore** path; to rebuild a preview cluster, restore to a single
> node and bootstrap a new static cluster around it. See
> [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) and
> [V0_9_RELEASE_NOTES.md](V0_9_RELEASE_NOTES.md).

> **AuraDB v0.8.0 hardens cluster-preview _recovery_ testing and adds operator
> runbooks. It is _not_ production HA; the multi-node cluster remains an
> experimental, opt-in preview, and single-node mode remains the recommended
> production mode.** v0.8.0 adds no new cluster architecture and no wire or storage
> change: it maps the recovery scenarios (leader loss, follower lag, snapshot
> needed, peer reconnect churn, partition/heal) to the existing cross-process
> preview suites rather than duplicating them, and adds operator runbooks for them.
> See [RUNBOOKS.md](RUNBOOKS.md), [CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md),
> and [V0_8_RELEASE_NOTES.md](V0_8_RELEASE_NOTES.md).

> **AuraDB v0.6.0 improves the controlled multi-node preview and validates
> fail-stop recovery. It is _not_ production HA. Single-node mode remains the
> recommended production mode.** Real AuraDB server processes can form a
> cross-process cluster, elect a leader, and replicate writes through Raft. This
> preview is **off by default** and is intended for local testing and early
> validation only. For diagnosing and recovering cluster mode, see
> [CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md).

> **New in v0.6.0.** A leader kill / automatic re-election **preview** (a stopped
> leader's term is taken over by the surviving majority; the old node rejoins as a
> follower and catches up); the first real **peer snapshot install over the wire**
> (a bounded single-message transfer for a follower that fell behind the
> compacted prefix); larger follower catch-up coverage; sharper fail-stop
> diagnostics (leader-change and snapshot-install counters); a published-image
> Docker Compose smoke (`AURADB_IMAGE`); and peer certificate/token rotation and
> cluster backup/restore runbooks. Leader kill and re-election are **fail-stop
> recovery preview** behavior — this is **not production HA** and not production
> automatic failover. See [V0_6_RELEASE_NOTES.md](V0_6_RELEASE_NOTES.md).

> **AuraDB v0.6.1 hardens snapshot install and published-cluster smoke for the
> controlled multi-node preview. It is not production HA. Single-node mode remains
> the recommended production mode.** v0.6.1 adds per-peer snapshot/lag diagnostics
> to `auradb cluster status --addr` and a live `auradb cluster doctor --addr`, the
> matching Prometheus/JSON metrics, dry-run cluster backup/restore planners
> (`auradb cluster backup-plan` / `restore-plan`), and multi-arch Docker images.
> See [CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md),
> [OBSERVABILITY.md](OBSERVABILITY.md), [OPERATIONS.md](OPERATIONS.md), and
> [CLI.md](CLI.md).

> **AuraDB v0.7.1 polishes connector ergonomics for the controlled multi-node
> preview (coordinated with Aura Connector v0.4.1). It is not production HA.
> Single-node mode remains the recommended production mode.** v0.7.1 is
> docs/conformance only: it adds no new database architecture and leaves the
> `not_leader` payload and the wire protocol (AWP 1) byte-for-byte unchanged from
> v0.7.0. Aura Connector v0.4.1 improves the client-side experience (clearer
> `AuraNotLeaderError` messages, a secure-by-default redirect, transaction-redirect
> docs). See [V0_7_1_RELEASE_NOTES.md](V0_7_1_RELEASE_NOTES.md) and
> [AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md).

> **AuraDB v0.7.0 adds connector cluster ergonomics for the controlled multi-node
> preview. It is not production HA. Single-node mode remains the recommended
> production mode.** The `not_leader` response now carries an additive, structured
> `not_leader` object (leader client address, leader/current node ids, term, role,
> and a usable `leader_hint`) so Aura Connector v0.4.x can redirect to the leader
> without parsing the message. The wire protocol (AWP 1) is unchanged and older
> clients ignore the new fields. See
> [AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md).

> **AuraDB v0.6.2 hardens repeated chaos and larger-state recovery behavior in
> the controlled multi-node preview. It is not production HA. Single-node mode
> remains the recommended production mode.** v0.6.2 adds repeated leader restart /
> re-election cycles, larger multi-model data-set recovery, multi-model snapshot
> install, peer reconnect-storm testing, deterministic network-interruption
> (partition/heal) simulations, and recovery diagnostics: `auradb cluster status
> --addr` now reports `leader_changes`, and `auradb cluster doctor --addr` warns
> on a peer reconnect storm and on repeated leader changes. Multi-node mode still
> requires both `enabled = true` and `experimental_multi_node = true`, membership
> is still static (no join/leave commands), and non-loopback cluster networking
> still requires TLS **and** a peer auth token. See
> [V0_6_2_RELEASE_NOTES.md](V0_6_2_RELEASE_NOTES.md).

AuraDB introduced cluster mode in v0.4.0 (an optional, durable replication path
built on a Raft consensus core) and hardened it in v0.4.1. v0.5.0 builds on that
groundwork by adding a real cross-process peer transport and Raft over it, so a
set of server processes can elect a leader and replicate writes to one another.
This document explains what the preview is, how it relates to the recommended
single-node production path, the two opt-ins and guardrails that gate it, how to
configure and run it, leader/follower behavior, and exactly where its boundaries
are.

Cluster mode is **disabled by default**. When it is disabled the engine behaves
exactly as it did in v0.3.1 — the write path is byte-for-byte the previous
single-node direct path, and the `[cluster]` configuration table is inert.

## The multi-node preview at a glance (v0.5.0)

Forming a **real cross-process cluster** requires two explicit opt-ins in
`[cluster]`:

```toml
[cluster]
enabled = true
experimental_multi_node = true
```

- Without `experimental_multi_node = true`, a non-empty `peers` list is
  **rejected at startup** — exactly the v0.4.1 behavior is preserved.
- Membership is **static**: every node declares every other node by `node_id`
  and `addr` (as `[[cluster.peers]]` entries or an inline
  `peers = [{ node_id = "...", addr = "..." }]`). There is **no join, leave, or
  dynamic membership.**
- Writes go to the **leader**; followers reject writes with a structured
  `not_leader` error carrying a leader hint, and the connection stays healthy.
  Followers serve reads from their locally replicated state — these are
  **eventually consistent and not linearizable** (they may be briefly stale
  relative to the leader), so they are not a production read-consistency guarantee.
  Send reads to the leader for fresh, correct results.
- The leader write path **blocks until a majority commits**; a minority cannot
  commit.
- A restarted follower replays its durable log and is brought current by the
  leader.

Single-node mode (cluster disabled, or cluster enabled with no peers) remains the
recommended production path. The multi-node preview is for local testing and
early validation only.

## What cluster mode is

When cluster mode is enabled, every data-plane commit is ordered through a
durable Raft log before it is applied to storage. The log entry's index becomes
the MVCC commit timestamp, so the commit order is fixed by consensus and is
identical on every replica derived from the same log. On restart, any entry that
was committed to the Raft log but not yet applied to storage is replayed, which
closes the crash window between a durable consensus commit and the storage apply.

This release ships **single-node cluster mode** (one node that is its own
majority, elects itself leader, and orders its own writes through the Raft log)
and, new in v0.5.0, an **experimental cross-process multi-node preview** in which
several server processes form a cluster over a real peer transport, elect a
leader, and replicate writes through Raft. The consensus core, the replicated
apply path, and the snapshot boundary are all real and tested. The multi-node
preview is off by default and gated behind two opt-ins (see
[The multi-node preview](#the-multi-node-preview-v050)).

### Single-node cluster vs. recommended single-node production mode

There are two distinct single-node configurations:

- **Single-node, non-cluster (recommended for production).** Cluster mode is
  disabled. Commits go straight to storage. This is the supported, recommended
  production path and the default.
- **Single-node cluster.** Cluster mode is enabled with no peers. Writes are
  ordered through the durable Raft log and replayed on restart. This faithfully
  exercises the replication path and is useful for validation and development.

A single-node cluster provides **no fault tolerance**: with a single node there
is no second replica to fail over to, so its availability is the same as
non-cluster single-node mode. It adds write-path overhead (every commit is framed
and appended to the Raft log) in exchange for exercising the replicated commit
ordering. For production, single-node non-cluster mode remains the recommended
path.

## The `[cluster]` configuration table

Cluster mode is configured through the `[cluster]` table in the server
configuration file. Every field:

| Field | Type | Default | Meaning |
| ----- | ---- | ------- | ------- |
| `enabled` | bool | `false` | Whether cluster (Raft) mode is enabled. When `false`, the rest of the table is inert and the engine uses the single-node direct write path. |
| `experimental_multi_node` | bool | `false` | **(v0.5.0)** Second opt-in required to form a real cross-process cluster. A non-empty `peers` list without this set to `true` is rejected at startup (preserving v0.4.1 behavior). |
| `allow_experimental_public_cluster` | bool | `false` | **(v0.5.0)** Permit a non-loopback cluster address (listen/advertise/peer). Setting this additionally **requires** peer TLS (`[cluster.tls]`) and a `peer_auth_token`. |
| `cluster_id` | string (hex) | `""` | Optional pinned 128-bit cluster id (32 hex digits). Identical on every node. Empty means use the persisted id, or generate one on bootstrap. Pinning enforces a specific identity; a mismatch is rejected. |
| `node_id` | string (hex) | `""` | Optional pinned non-zero 64-bit node id (16 hex digits). Distinct per node. Empty means use the persisted id, or generate one on init. |
| `listen_addr` | string (`host:port`) | `127.0.0.1:7172` | Address the cluster (Raft) transport binds to. Must be loopback unless `allow_experimental_public_cluster = true`. |
| `advertise_addr` | string (`host:port`) | `127.0.0.1:7172` | Address advertised to peers (may differ from `listen_addr` behind NAT). |
| `bootstrap` | bool | `true` | Whether this node bootstraps a brand-new cluster. |
| `peer_auth_token` | string | `""` | **(v0.5.0)** Shared peer authentication token verified in the `PeerHello` handshake. Required when `allow_experimental_public_cluster = true`. |
| `peers` | list of `{ node_id, addr }` | `[]` | **(v0.5.0)** Static membership: every other node, by id and cluster address. A non-empty list requires `experimental_multi_node = true`. |
| `[cluster.tls]` | disabled | **(v0.5.0)** Peer-transport TLS (`cert_path`, `key_path`, `ca_path`). Required when `allow_experimental_public_cluster = true`. |

A single-node cluster configuration looks like this:

```toml
[cluster]
enabled = true
cluster_id = ""        # use/persist the existing id, or generate on bootstrap
node_id = ""           # use/persist the existing id, or generate on init
listen_addr = "127.0.0.1:7172"
advertise_addr = "127.0.0.1:7172"
bootstrap = true
peers = []             # empty: single-node cluster
```

A complete example ships at `examples/auradb.cluster.local.toml`. The three-node
loopback preview ships at `examples/cluster/node{1,2,3}.toml` and a Docker
Compose preview (which requires peer TLS and a token) at
`examples/cluster/docker/`. Validate any configuration offline before starting
the server:

```sh
auradb config validate --config examples/auradb.cluster.local.toml
```

### Guardrails (all fail closed)

The multi-node preview is gated so a cluster is never *appeared* to be formed
when it is not, and an unsafe transport is never opened silently:

- A non-empty `peers` list **without** `experimental_multi_node = true` is
  rejected at startup.
- Any non-loopback cluster address (listen / advertise / peer) is rejected
  **unless** `allow_experimental_public_cluster = true`, which **additionally**
  requires peer TLS (`[cluster.tls]` with `cert_path` / `key_path` / `ca_path`)
  and a `peer_auth_token`.
- Membership is static; a duplicate peer, a peer pointing at this node, or a
  malformed `host:port` is rejected.

See [SECURITY.md](SECURITY.md) and [CONFIGURATION.md](CONFIGURATION.md).

## On-disk layout

Cluster identity and the Raft log live under `<data_dir>/cluster/`:

```text
cluster/
  node.json        # this node's stable id (and the version that created it)
  cluster.json     # the cluster this node belongs to
  raft-log.bin     # append-only, framed, CRC32-checksummed Raft log entries
  raft-state.json  # the durable Raft hard state (term, vote, commit index)
```

`node.json` and `cluster.json` each carry a `format_version`. A file written by a
newer AuraDB (a higher `format_version`) is rejected rather than guessed at:
AuraDB fails closed on unknown future formats. The Raft log and hard state files
are described in [RAFT.md](RAFT.md).

The regular storage segments are unchanged from v0.3.1 and live where they always
did; the Raft log is a separate durable log. See
[STORAGE_ENGINE.md](STORAGE_ENGINE.md).

## CLI commands

The `auradb cluster` subcommands inspect and prepare a data directory's cluster
state offline (without standing up a running node):

```sh
# Create stable node and cluster identity if not already present.
auradb cluster init --data-dir .local/auradb

# Show local cluster metadata for a data directory.
auradb cluster status --data-dir .local/auradb
auradb cluster status --data-dir .local/auradb --json

# List configured cluster peers.
auradb cluster peers --data-dir .local/auradb

# Validate cluster configuration and identity offline.
auradb cluster doctor --data-dir .local/auradb

# Bootstrap a brand-new single-node cluster identity.
auradb cluster bootstrap --data-dir .local/auradb
```

`auradb init` now also creates node identity, and `auradb status --json` and
`auradb doctor` include the cluster fields. See [CLI.md](CLI.md).

New in v0.5.0, three **live** subcommands query a running server over its client
address (and accept `--json`, `--token`, `--tls-ca`, `--server-name`):

```sh
# Report the leader a running server currently recognizes.
auradb cluster leader --addr 127.0.0.1:7171

# Block until a server reports a recognized leader, or is ready.
auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30
auradb cluster wait-ready  --addr 127.0.0.1:7171 --timeout-secs 30
```

The membership operations `join`, `leave`, and `step-down` are **intentionally
not provided**, because membership changes are not implemented in this release.

## Running the three-node loopback preview

The validated preview path is three local processes on loopback with no TLS. The
configs ship at `examples/cluster/node{1,2,3}.toml`; client ports are
`7171`/`7181`/`7191` and cluster (Raft) ports are `7172`/`7182`/`7192`.

```sh
# Three terminals from the repository root:
auradb server --config examples/cluster/node1.toml
auradb server --config examples/cluster/node2.toml
auradb server --config examples/cluster/node3.toml

# Wait for an election, then see who won and the per-peer state.
auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30
auradb cluster leader      --addr 127.0.0.1:7171
auradb cluster status      --addr 127.0.0.1:7171 --json
```

Write through the leader's client address (use the Aura Connector or any AWP
client). A write sent to a follower returns `not_leader` with a leader hint. To
watch catch-up: stop one follower (a 2/3 majority remains, so writes continue),
keep writing through the leader, then restart the follower with the same config —
it replays its durable log and the leader brings it current.

A Docker Compose preview that runs over a Docker bridge network (and therefore
requires `allow_experimental_public_cluster = true`, peer TLS, and a shared
token) ships at `examples/cluster/docker/` with `docker-compose.cluster.yml`. See
[examples/cluster/README.md](../examples/cluster/README.md).

## Writes and reads

### Leader-only writes and `not_leader`

Only the leader accepts writes. A write that reaches a non-leader is rejected with
the `not_leader` error code, which carries a hint identifying the current leader
when one is known; the connection stays healthy afterward. In the multi-node
preview the leader appends to its Raft log, replicates via AppendEntries, and the
write path **blocks until a majority commits** — a minority cannot commit. Every
node applies committed entries to its engine. In a single-node cluster the sole
node is always the leader, so writes are accepted as usual.

The `not_leader` error code is additive on the wire. The Aura Wire Protocol
version is unchanged at AWP 1; an Aura Connector 0.3.x client maps an unknown
error code safely. See [AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md).

### Read policy

The **recommended, supported path is to send reads to the leader** — they are fresh
and correct. Followers also serve reads from their locally replicated state, but
these are **eventually consistent and not linearizable**: they may be briefly stale
relative to the leader, and the preview does **not** offer linearizable reads or any
stale-read consistency tuning — those are not implemented and are not claimed as a
production guarantee. In a single-node cluster, leader-served reads are simply reads
against the only node. This applies uniformly to all reads, including ranked search
(BM25, vector, hybrid) — see [SEARCH_AND_RANKING.md](SEARCH_AND_RANKING.md).

## Cluster health and status

When cluster mode is enabled, the health/status report gains an additive
`cluster` section with these fields: `node_id`, `cluster_id`, `role`, `term`,
`leader_id`, `commit_index`, `applied_index`, `last_log_index`, `peer_count`,
`single_node`, and `replication_lag_entries`. New in v0.5.0, the section also
carries `preview_multi_node` (bool), `quorum_available` (bool), and `peers` (an
array of `{ node_id, addr, connected, match_index, next_index }`). These are
additive AWP fields; older clients ignore them. The field is purely additive
JSON; the wire protocol version is unchanged. See
[OBSERVABILITY.md](OBSERVABILITY.md) for the field meanings and the corresponding
metrics.

## The multi-node preview (v0.5.0)

The Raft consensus core (leader election, log replication, log repair, commit
advancement) and the replicated apply path are validated both through
deterministic in-process tests and, in v0.5.0, across **real server processes**
over a dedicated peer transport:

- **Cross-process peer transport.** A dedicated cluster socket carries Raft
  messages. Each frame is magic-tagged (`APR1`), protocol-version-tagged (v1),
  length-delimited, and CRC32-checksummed, with a 16 MiB payload-size limit. A
  connection opens with a `PeerHello` handshake verifying the protocol version,
  the cluster id, the peer's node id (against static membership), and a shared
  token. Wrong-cluster, unknown-node, duplicate-node, and bad-token connections
  are rejected with a structured `PeerError`. Reconnect uses bounded backoff
  (50 ms .. 2 s) and shutdown is graceful.
- **Snapshot install is not implemented** and is answered with a structured
  *unsupported* response — never silently ignored. See [RAFT.md](RAFT.md).
- **commit_ts = commit_ts_base + raft_log_index**, unchanged from v0.4.x.

The preview is gated behind `enabled = true` **and** `experimental_multi_node =
true`, and any non-loopback address fails closed unless
`allow_experimental_public_cluster = true` (which then requires peer TLS and a
token). See [RAFT.md](RAFT.md), [REPLICATION.md](REPLICATION.md), and
[TESTING.md](TESTING.md).

## Limitations

This release deliberately does not provide, and does not claim:

- A production-grade distributed database. Single-node non-cluster mode is the
  recommended production path; the multi-node preview is experimental.
- Production multi-node clustering or production-grade peer networking.
- Fault tolerance from a single-node cluster (it has the same availability as a
  single non-cluster node).
- Automatic failover.
- Linearizable reads or a production read-consistency guarantee (followers serve
  only eventually-consistent, non-linearizable reads; send reads to the leader).
- Distributed transactions, sharding, or multi-region deployment.
- Dynamic membership (`join` / `leave` / `step-down`) or joint consensus;
  membership is static.
- Streaming snapshot transfer between nodes (snapshot install is answered as
  unsupported; only the snapshot boundary is defined; see
  [REPLICATION.md](REPLICATION.md)).

Cluster mode changes nothing about single-node isolation semantics: AuraDB
provides single-node snapshot isolation, not serializable or distributed
isolation. See [TRANSACTIONS.md](TRANSACTIONS.md).
