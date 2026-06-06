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

### Live cluster diagnostics (v0.5.1)

For a running server, `auradb cluster status --addr <host:port>` queries it for
live cluster state:

```bash
auradb cluster status --addr 127.0.0.1:7171 --json
```

It reports `role`, `term`, `leader_id`, the leader's client address
(`leader_client_addr`, when a peer declared a `client_addr`), `quorum_available`,
`commit_index` / `applied_index` / `last_log_index`, `replication_lag_entries`,
and a `peers` array with each peer's `connected` state, `connect_attempts`, and
`match_index` / `next_index`. Use it to spot an unreachable peer (its
`connected` is `false` and `connect_attempts` keeps rising), a lost quorum
(`quorum_available: false`), or a lagging follower (a large
`replication_lag_entries` or a `match_index` far behind the leader's
`last_log_index`).

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
not_leader: this node (<hex>) is not the leader; current leader is node <hex>
(client address <host:port>); retry the write against the leader
```

The error carries a stable `not_leader` code, the current node id, the known
leader id (and the leader's client address when a peer declared a `client_addr`),
and retry/redirect guidance in the message; the wire error payload also marks it
`retryable: true`. When no leader is known yet, the message says so and advises a
short backoff. The same client connection stays usable after a `not_leader`
response.

In single-node cluster mode the sole node is always the leader, so you should not
see `not_leader` in normal operation. In the multi-node preview, **route writes
to the leader's client address.** A write to a follower returns `not_leader` with
a hint; find the leader with `auradb cluster leader --addr <client-addr>` (or the
`cluster` section of `auradb status --json`) and send writes there. If no leader
is known yet, the node has not finished its election — wait with `auradb cluster
wait-leader`. Aura Connector 0.3.x surfaces `not_leader` as a normal server
error; it does not crash, does not retry forever, and does not drop auth/TLS
state.

**With Aura Connector 0.4.x** the `not_leader` response maps to a dedicated
`AuraNotLeaderError` that exposes the leader address and routing hints. Recover by
either resolving the leader first (`auradb cluster leader --addr <node> --json`)
and connecting there, or catching the error and calling
`client.connect_to_leader(exc)` (preserves token auth and TLS) or the opt-in
bounded `client.with_leader_redirect()`. v0.4.1 renders a clearer message (the
node reached, the leader address, the redirect call) and refuses a redirect that
would silently drop TLS. Transactions are never auto-redirected — restart the
transaction on the leader. See the connector's `examples/auradb_leader_redirect.py`.

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

## A follower that needs a snapshot (v0.6.1)

When a follower has fallen below the leader's compacted prefix, AppendEntries can
no longer serve it and the leader must ship a snapshot. v0.6.1 makes this visible
in the live status report:

```bash
auradb cluster status --addr 127.0.0.1:7171 --json
auradb cluster doctor --addr 127.0.0.1:7171 --json
```

- In `cluster status --addr`, the peer's `catch_up_state` reads `snapshot_needed`
  (an install is required) or `snapshot_installing` (one is in progress), and
  `needs_snapshot` is `true`. Cluster-level snapshot diagnostics report the last
  installed boundary, last install time, last error (the rejection reason), bytes
  sent, bytes installed, the in-progress gauge, and the needed-total.
- `cluster doctor --addr` fetches live health and emits a warning for a follower
  that needs a snapshot.
- Watch the metrics `auradb_cluster_snapshot_needed_total`,
  `auradb_cluster_snapshot_in_progress`, `auradb_cluster_snapshot_bytes_sent_total`,
  `auradb_cluster_snapshot_bytes_installed_total`, and
  `auradb_cluster_snapshot_last_error` (a 0/1 gauge; the textual reason is in the
  `cluster status` JSON, not a metric label). See [OBSERVABILITY.md](OBSERVABILITY.md).

A rising sent/installed pair during recovery is the healthy signal that a snapshot
install is doing the catch-up. `auradb_cluster_snapshot_last_error` set to `1`
(with a reason in the status JSON) points at a rejected install — an oversized
snapshot, a wrong cluster id, a bad digest, or a future format.

## A lagging follower (v0.6.1)

A follower that trails but is still within the leader's retained log is caught up
by AppendEntries. v0.6.1 quantifies the lag in the live status report:

- In `cluster status --addr`, the peer's `lag_entries` is how far its match index
  trails the leader, and `catch_up_state` reads `probing` or `normal` while it
  closes the gap and `caught_up` once converged.
- `cluster doctor --addr` emits warnings for a lagging follower and for quorum at
  the minimum or quorum lost.

Confirm the follower converges (`lag_entries` falls toward zero,
`catch_up_state` reaches `caught_up`). A follower whose lag keeps growing while
others converge usually means it is unreachable (`connected: false`) or has
fallen below the compacted prefix (see the snapshot section above).

## Repeated fail-stop and leadership instability (v0.6.2)

A single clean failover is expected: kill the leader, the majority elects a new
one, the old node rejoins as a follower and catches up. **Repeated** leadership
changes are not — they point to instability rather than a one-off recovery.

- `cluster status --addr` now reports `leader_changes`, the cumulative count of
  leadership changes this node has observed since it started. A value that keeps
  climbing across a steady cluster is the signal to investigate.
- `cluster doctor --addr` warns (**"leadership has changed N times … repeated
  leader changes suggest instability"**) once that count crosses a threshold.

Common causes and what to check:

- **An overloaded or CPU-starved leader** misses its own heartbeat deadlines, so
  followers time out and campaign. Check leader CPU and `heartbeat_latency_ms`.
- **A flaky peer link** drops heartbeats intermittently. Check per-peer
  `connected` / `connect_attempts` and the reconnect-storm guidance below.
- **Election-timeout contention** under heavy load. Repeated `election_timeouts`
  with no committed progress is the tell.

After the cause is fixed, `leader_changes` stops climbing and a single stable
leader holds (`role: leader` on exactly one node, recognized by a majority).

## A peer reconnect storm (v0.6.2)

A peer that flaps — disconnecting and reconnecting repeatedly — shows up as a
**rising `connect_attempts` against a peer that is still `connected: false`**.
The outbound dialer uses bounded backoff (it does not spin), so the attempt count
rises slowly; a count in the tens against a peer that never connects means the
peer's address is wrong, its listener is down, or its peer auth/TLS is
misconfigured.

- `cluster doctor --addr` warns (**"peer … is in a reconnect storm: N connection
  attempts and still not connected"**) once the attempt count crosses a
  threshold while the peer remains disconnected.
- Replication resumes automatically the moment the peer becomes reachable again,
  and a follower that was flapping catches up by AppendEntries (or a snapshot
  install if it fell below the compacted prefix) with **no duplicate apply**.

Check the peer's `listen_addr`, that its process is up and bound, and that the
`peer_auth_token` and cluster TLS material match across nodes (see
[SECURITY.md](SECURITY.md)).

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

**A follower behind the compacted prefix (v0.6.0).** If a follower has fallen so
far behind that the entries it needs were compacted away, the leader can no
longer serve it with AppendEntries. In v0.6.0 the leader ships a **bounded,
single-message peer snapshot install** to bring it current, then resumes
AppendEntries. Watch the metrics:

- `auradb_cluster_snapshots_sent_total` rising on the leader and
  `auradb_cluster_snapshots_installed_total` rising on the follower means a
  snapshot install is doing the catch-up — expected and healthy.
- `auradb_cluster_snapshots_rejected_total` rising on the follower means an
  install was refused (oversized snapshot beyond `MAX_SNAPSHOT_BYTES` = 8 MiB,
  a wrong cluster id, a bad payload digest, or a newer manifest/storage format).
  Check the peers share a cluster id and run compatible builds; a dataset whose
  snapshot exceeds the size limit cannot be caught up by the single-message
  preview install.

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
