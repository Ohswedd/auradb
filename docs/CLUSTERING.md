# Clustering

> **AuraDB v0.4.1 hardens the Raft groundwork introduced in v0.4.0. Multi-node
> server deployment remains experimental and disabled by default. Single-node
> mode remains the recommended production mode.** v0.4.1 adds Raft log compaction
> boundaries, snapshot restore hardening, cluster-metadata corruption handling,
> stronger peer-configuration validation, and operational diagnostics. For
> diagnosing and recovering cluster mode, see
> [CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md).

AuraDB v0.4.0 introduces cluster mode: an optional, durable replication path
built on a Raft consensus core. This document explains what cluster mode is in
this release, how it relates to the recommended single-node production path, how
to configure and operate it, and exactly where its boundaries are.

Cluster mode is **disabled by default**. When it is disabled the engine behaves
exactly as it did in v0.3.1 — the write path is byte-for-byte the previous
single-node direct path, and the `[cluster]` configuration table is inert.

## What cluster mode is

When cluster mode is enabled, every data-plane commit is ordered through a
durable Raft log before it is applied to storage. The log entry's index becomes
the MVCC commit timestamp, so the commit order is fixed by consensus and is
identical on every replica derived from the same log. On restart, any entry that
was committed to the Raft log but not yet applied to storage is replayed, which
closes the crash window between a durable consensus commit and the storage apply.

This release ships and wires up **single-node cluster mode** for the server: one
node that is its own majority, elects itself leader, and orders its own writes
through the Raft log. The consensus core, the replicated apply path, and the
snapshot boundary are all real and tested. Multi-node server deployment is
**experimental and not enabled** in this release (see
[Multi-node status](#multi-node-status-experimental)).

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
| `cluster_id` | string (hex) | `""` | Optional pinned 128-bit cluster id (32 hex digits). Empty means use the persisted id, or generate one on bootstrap. Pinning enforces a specific identity; a mismatch with the persisted id is rejected. |
| `node_id` | string (hex) | `""` | Optional pinned non-zero 64-bit node id (16 hex digits). Empty means use the persisted id, or generate one on init. |
| `listen_addr` | string (`host:port`) | `127.0.0.1:7172` | Address the cluster (Raft) transport binds to. Must be loopback in this release unless `--allow-insecure-bind` is passed. |
| `advertise_addr` | string (`host:port`) | `127.0.0.1:7172` | Address advertised to peers (may differ from `listen_addr` behind NAT). |
| `bootstrap` | bool | `true` | Whether this node bootstraps a brand-new single-node cluster. |
| `peers` | list of `host:port` | `[]` | Peer cluster addresses for multi-node deployments. **Configuring any peer is rejected at server startup in this release.** |

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

A complete example ships at `examples/auradb.cluster.local.toml`. Validate any
configuration offline before starting the server:

```sh
auradb config validate --config examples/auradb.cluster.local.toml
```

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

The membership operations `join`, `leave`, and `step-down` are **intentionally
not provided**, because membership changes are not implemented in this release.

## Writes and reads

### Leader-only writes and `not_leader`

Only the leader accepts writes. A write that reaches a non-leader is rejected with
the `not_leader` error code, which carries a hint identifying the current leader
when one is known. In a single-node cluster the sole node is always the leader, so
writes are accepted as usual.

The `not_leader` error code is additive on the wire. The Aura Wire Protocol
version is unchanged at AWP 1; an Aura Connector 0.3.x client maps an unknown
error code safely. See [AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md).

### Read policy

Reads are served by the leader. This release does **not** offer linearizable
reads, follower reads, or stale-read tuning — those are not implemented and are
not claimed. In a single-node cluster, leader-served reads are simply reads
against the only node.

## Cluster health and status

When cluster mode is enabled, the health/status report gains an additive
`cluster` section with these fields: `node_id`, `cluster_id`, `role`, `term`,
`leader_id`, `commit_index`, `applied_index`, `last_log_index`, `peer_count`,
`single_node`, and `replication_lag_entries`. The field is purely additive JSON;
the wire protocol version is unchanged. See [OBSERVABILITY.md](OBSERVABILITY.md)
for the field meanings and the corresponding metrics.

## Multi-node status (experimental)

The Raft consensus core (leader election, log replication, log repair, commit
advancement) and the replicated apply path are implemented and validated through
deterministic in-process tests. Cross-process, multi-node *server* deployment is
**not** part of this release:

- Configuring any `peers` in `[cluster]` is **rejected at server startup** (fail
  closed). The server will not appear to form a multi-node cluster.
- The cluster transport is unauthenticated in this release, so a non-loopback
  cluster `listen_addr` is rejected unless `--allow-insecure-bind` is explicitly
  passed. See [SECURITY.md](SECURITY.md).

Multi-node consensus is exercised entirely by the deterministic in-memory test
harness described in [RAFT.md](RAFT.md) and [TESTING.md](TESTING.md).

## Limitations

This release deliberately does not provide, and does not claim:

- A production-grade distributed database. Single-node non-cluster mode is the
  recommended production path.
- Multi-node server deployment. Configuring peers is rejected at startup.
- Fault tolerance from a single-node cluster (it has the same availability as a
  single non-cluster node).
- Automatic failover.
- Linearizable reads or follower reads.
- Distributed transactions, sharding, or multi-region deployment.
- An authenticated cluster transport (cluster mode is loopback-only here).
- Membership changes (`join` / `leave` / `step-down`) or joint consensus.
- Streaming snapshot transfer between nodes (only the snapshot boundary is
  defined; see [REPLICATION.md](REPLICATION.md)).

Cluster mode changes nothing about single-node isolation semantics: AuraDB
provides single-node snapshot isolation, not serializable or distributed
isolation. See [TRANSACTIONS.md](TRANSACTIONS.md).
