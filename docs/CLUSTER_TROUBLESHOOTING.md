# Cluster troubleshooting

This guide covers diagnosing and recovering AuraDB's optional cluster mode. It
applies to **single-node cluster mode**, which is the only cluster deployment
enabled in this release.

> **Scope and honesty.** Multi-node server deployment is **experimental and
> disabled by default**. Configuring peers is rejected at startup. A single-node
> cluster orders writes through a durable local Raft log but provides **no fault
> tolerance** — it is one process. There is no automatic failover, no linearizable
> follower reads, and no distributed transactions. Single-node (non-cluster) mode
> remains the recommended production path.

## Inspecting node and cluster identity

Identity lives under `<data_dir>/cluster/` as `node.json` and `cluster.json`.
Inspect it offline:

```bash
auradb cluster status --data-dir <dir> --json
```

Fields:

- `node_id` — this node's stable 64-bit id (hex).
- `cluster_id` — the cluster's stable 128-bit id (hex).
- `initialized` — whether identity exists on disk.
- `single_node` / `peer_count` — single-node clusters report `peer_count: 0`.

The runtime role/term of a *running* server is reported by
`auradb status --addr <host:port> --json` under the `cluster` section, not by the
offline `cluster status`.

## Validating cluster metadata

```bash
auradb cluster doctor --data-dir <dir> --json
auradb doctor --data-dir <dir> --json   # also validates cluster metadata
```

`doctor` validates the cluster configuration (fails closed on an invalid one) and
reports warnings. Loading metadata also validates its on-disk format, so an
**unknown future format is rejected** here rather than silently opened.

Common doctor warnings:

| Warning | Meaning |
| ------- | ------- |
| `cluster mode is enabled but no identity is initialized` | Run `auradb cluster bootstrap` (or `auradb cluster init`). |
| `cluster mode is enabled with no peers and bootstrap = false` | A node with `bootstrap = false` needs at least one peer; either bootstrap a new single-node cluster or fix the config. |
| `cluster listen_addr … is not loopback and cluster transport is unauthenticated` | Cluster transport has no authentication in this release; bind loopback, or pass `--allow-insecure-bind` to accept the risk explicitly. |

## What `not_leader` means

A write is only accepted by the **leader**. If a node is a follower (or no leader
is known), a write returns a structured `not_leader` error with a leader hint:

```text
not_leader: this node is not the leader; current leader is node <hex>
```

In single-node cluster mode the sole node is always the leader, so you should not
see `not_leader` in normal operation. If you do, the node has not yet completed
its election on startup — retry after it is ready (`auradb status` reports
readiness). Aura Connector 0.3.x surfaces this as a normal server error; it does
not crash, does not retry forever, and does not drop auth/TLS state.

## What peer rejection means in this release

Any non-empty `cluster.peers` list is rejected at startup:

```text
multi-node cluster deployment is experimental and not enabled in this release;
run a single-node cluster (no peers) or disable [cluster]
```

This is intentional. The Raft and replication core is validated by deterministic
in-process tests, but cross-process transport and its security story are not
production-ready. Configuration is also validated for duplicate peers and a peer
that points at the node's own address — both are configuration errors.

## Compacting the Raft log

Over time the durable Raft log grows. Once the engine has durably applied a
prefix, it can be compacted away:

```bash
# Preview first (nothing is modified):
auradb cluster compact-log --data-dir <dir> --dry-run --json

# Then compact:
auradb cluster compact-log --data-dir <dir> --json
```

Compaction never runs ahead of durability: it discards only entries at or below
both the committed and applied indices, records the last included index/term in
`raft-compaction.json`, and leaves the retained suffix intact. Reads before the
retained prefix fail closed with a `Compacted` error rather than returning a wrong
answer.

## Inspecting snapshot manifests

```bash
auradb snapshot create  --data-dir <dir> --output <file>
auradb snapshot inspect --input <file>
```

`inspect` prints the manifest (format version, cluster/node id, last included
index/term, storage-format version, collection/record counts, digest) and
verifies the payload digest (`integrity: ok`).

## Recovering from corrupt cluster metadata

AuraDB **fails closed** on corrupt or partial cluster metadata rather than
guessing. If `auradb cluster status` or server startup reports corrupt or
incomplete identity:

1. **Do not delete data.** First inspect `<data_dir>/cluster/node.json` and
   `cluster.json`. A partial state (one file present, the other missing) is
   reported as an identity conflict.
2. If only the cluster identity files are damaged but the engine data is intact,
   the safest recovery is to **restore from a backup** taken with
   `auradb snapshot create` or `auradb dump`.
3. A `raft-compaction.json` that disagrees with the retained log, or a future
   format version, is reported as corruption — restore from backup rather than
   editing it by hand.

## When to restore from backup

Restore from a known-good snapshot or dump when:

- cluster or Raft metadata is reported corrupt and the cause is unknown;
- the storage engine itself reports corruption on open;
- you need to move data to a clean directory.

```bash
auradb snapshot restore --input <file> --data-dir <new-dir>
# Restore refuses to overwrite a non-empty directory unless you pass --force.
```

Restore is atomic: it builds into a staging directory, validates, and only then
swaps into place, so a failed restore never corrupts an existing directory.

## Why multi-node server deployment is not production-ready yet

Cross-process Raft transport, its authentication/TLS story, membership changes,
snapshot shipping, and automatic failover are not implemented. Enabling peers is
rejected at startup precisely so a cluster is never *appeared* to be formed when
it is not. Treat single-node mode as the production path; use single-node cluster
mode only to exercise the durable Raft write path.

## Metrics to watch

When cluster mode is enabled, the server exposes (and `auradb status --json`
reports) cluster gauges:

- `role`, `term`, `commit_index`, `applied_index`, `last_log_index`;
- `replication_lag_entries` (committed minus applied — should be ~0 on a healthy
  single node);
- `leader_changes`, `votes_granted`, `append_entries_sent` / `received`;
- `apply_errors` (should stay at 0; any increase indicates a replay/apply problem).

## Safe single-node cluster usage

- Keep `cluster.peers` empty.
- Bind the cluster listen address to loopback.
- Take regular snapshots/backups; compaction relies on applied state being durable.
- Treat the single node as having no fault tolerance — pair it with normal backups
  and process supervision, not with an expectation of automatic failover.
