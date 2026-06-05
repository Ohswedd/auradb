# Cluster troubleshooting

This guide covers diagnosing and recovering AuraDB's optional cluster mode. It
applies to **single-node cluster mode** and to the **v0.5.0 experimental
multi-node preview**.

> **AuraDB v0.5.0 introduces a controlled, experimental multi-node server
> preview. Single-node mode remains the recommended production mode.** The
> preview is off by default and gated behind two `[cluster]` opt-ins. A
> single-node cluster orders writes through a durable local Raft log but provides
> **no fault tolerance** — it is one process. The preview has **no automatic
> failover**, **no linearizable or follower reads** (followers reject reads), and
> **no distributed transactions**. It is for local testing and early validation
> only.

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
| `cluster listen_addr … is not loopback` | A non-loopback cluster address requires `allow_experimental_public_cluster = true` plus peer TLS and a token; otherwise bind loopback. |

The doctor warnings available for the preview cover: no leader, no quorum, a
follower lagging, a peer unreachable, a cluster id mismatch, a node id mismatch,
insecure peer transport, preview mode enabled, public cluster allowed, and
snapshot install unsupported.

## What `not_leader` means

A write is only accepted by the **leader**. If a node is a follower (or no leader
is known), a write returns a structured `not_leader` error with a leader hint:

```text
not_leader: this node is not the leader; current leader is node <hex>
```

In single-node cluster mode the sole node is always the leader, so you should not
see `not_leader` in normal operation. In the multi-node preview, **route writes
to the leader's client address.** A write to a follower returns `not_leader` with
a hint; find the leader with `auradb cluster leader --addr <client-addr>` (or the
`cluster` section of `auradb status --json`) and send writes there. If no leader
is known yet, the node has not finished its election — wait with `auradb cluster
wait-leader`. Aura Connector 0.3.x surfaces `not_leader` as a normal server
error; it does not crash, does not retry forever, and does not drop auth/TLS
state.

## Multi-node preview troubleshooting (v0.5.0)

The v0.5.0 preview forms a real cross-process cluster when both opt-ins are set
(`enabled = true` and `experimental_multi_node = true`). Use the live commands
against a running server's client address (they accept `--json`, `--token`,
`--tls-ca`, `--server-name`):

```bash
auradb cluster wait-ready  --addr 127.0.0.1:7171 --timeout-secs 30
auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30
auradb cluster leader      --addr 127.0.0.1:7171 --json
auradb status              --addr 127.0.0.1:7171 --json   # per-peer cluster state
```

The `cluster` section of `auradb status --json` reports `preview_multi_node`,
`quorum_available`, and a `peers` array of `{ node_id, addr, connected,
match_index, next_index }`. Common situations:

| Symptom | Likely cause and action |
| ------- | ----------------------- |
| Writes return `not_leader` | You are talking to a follower. Route to the leader's client address (`cluster leader`). |
| `quorum_available: false`, no leader | Fewer than a majority of nodes are reachable. A minority **cannot** commit by design. Start enough nodes to form a majority. |
| A peer shows `connected: false` | Peer unreachable: check the process is up, the cluster (Raft) address/port is correct, and (for a public cluster) that peer TLS and the token match. |
| A follower lags (`match_index` well behind the leader) | It is catching up; the leader replicates via AppendEntries. Confirm it converges; if it was restarted it first replays its durable log, then the leader brings it current. |
| Handshake rejected with a `PeerError` | Cluster id mismatch, an unknown or duplicate node id, a bad token, or a protocol-version mismatch — verify every node shares one `cluster_id`, declares a distinct `node_id` in the static membership, and (public cluster) shares the `peer_auth_token`. |
| A snapshot-install request returns *unsupported* | Expected: snapshot install over the wire is not implemented and is answered with a structured unsupported response, never silently ignored. |

The peer transport is frame-checked (magic `APR1`, protocol version v1,
length-delimited, CRC32, 16 MiB cap), and any non-loopback cluster address fails
closed unless `allow_experimental_public_cluster = true` (which then requires
peer TLS and a token). Membership is static; a duplicate peer, a self-peer, or a
malformed `host:port` is rejected at startup.

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

## Why the multi-node preview is not production-ready yet

The v0.5.0 cross-process Raft transport, leader election, and replication are
real, but dynamic membership, snapshot shipping over the wire, automatic
failover, and production-grade peer networking are not. The preview is an
experimental, opt-in path for local testing and early validation. Treat
single-node mode as the production path; use the multi-node preview only to
explore cross-process replication.

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
