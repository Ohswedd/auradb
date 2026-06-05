# AuraDB v0.5.0 release notes

**Theme: a controlled, experimental multi-node server preview.**

AuraDB v0.5.0 is the first release in which real AuraDB **server processes** can
form a cluster across processes (and hosts): they elect a leader over a dedicated
peer transport and replicate writes through a Raft log. This is an explicit,
**experimental preview** intended for local testing and early validation only.

**Single-node mode remains the recommended production path.** Cross-process peer
networking is **off by default** and must be turned on with two explicit opt-ins:

```toml
[cluster]
enabled = true
experimental_multi_node = true
```

All v0.4.1 behavior, the on-disk storage format, and the Aura Wire Protocol are
preserved. **Aura Connector 0.3.x remains compatible** — no connector release is
required.

## At a glance

- **Cross-process peer transport.** A dedicated cluster socket carries Raft
  messages between processes. Every frame is magic- and version-tagged,
  length-delimited, and CRC32-checksummed with a payload-size limit. A connection
  opens with a versioned `PeerHello` handshake that verifies the protocol
  version, the cluster id, the peer's node id (against the static membership), and
  a shared authentication token. Unknown, duplicate, or wrong-cluster peers are
  rejected with a structured `PeerError`. Snapshot install is **not** implemented
  and is answered with a structured *unsupported* response rather than silently
  ignored.
- **Static multi-node membership.** Peers are declared explicitly
  (`[[cluster.peers]]` with `node_id` and `addr`). There is no join, leave, or
  dynamic membership.
- **Secure peer transport baseline.** Loopback-only peer networking may run
  without TLS for the local preview. Any non-loopback peer address **fails
  closed** unless `allow_experimental_public_cluster = true`, which additionally
  requires peer TLS and a peer authentication token.
- **Real leader election and replicated writes across processes.** The leader
  appends to its Raft log, replicates via AppendEntries, commits on majority, and
  every node applies committed entries to its engine. A minority cannot commit.
- **Follower catch-up after restart.** A restarted follower replays its durable
  log and is brought current by the leader.
- **Clear leader/follower client behavior.** Writes go to the leader; a follower
  rejects writes with a structured `not_leader` error and a leader hint, and the
  connection stays healthy.
- **Cluster status and diagnostics across peers.** Live status includes per-peer
  connection state, match/next index, replication lag, quorum availability, and a
  preview-mode flag.
- **A three-node local example** (`examples/cluster/`) and a Docker Compose
  cluster (`docker-compose.cluster.yml`).

## New CLI commands

```bash
# Report the leader recognized by a running server.
auradb cluster leader --addr 127.0.0.1:7171

# Block until a server reports a recognized leader (or a server is ready).
auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30
auradb cluster wait-ready  --addr 127.0.0.1:7171 --timeout-secs 30
```

All three accept `--json` and the usual `--token` / `--tls-ca` / `--server-name`
flags. `auradb status --json` now includes per-peer cluster state when run against
a multi-node node.

## New metrics

`auradb_peer_connected`, `auradb_peer_replication_lag_entries`,
`auradb_raft_elections_total`, `auradb_raft_election_timeouts_total`,
`auradb_raft_append_entries_failures_total`, `auradb_raft_heartbeat_latency_ms`,
and `auradb_cluster_quorum_available`.

## Compatibility

- **Storage format:** unchanged from v0.4.x. Existing data opens as-is.
- **Cluster metadata and Raft log:** the v0.4.x on-disk identity, durable Raft
  log, and compaction metadata open unchanged; enabling the preview on upgraded
  data works.
- **Aura Wire Protocol:** unchanged. The cluster health payload gains additive,
  optional fields (per-peer state, preview/quorum flags); older clients ignore
  them.
- **Aura Connector:** 0.3.x remains compatible. A connector connects to the
  leader's client address; a write sent to a follower returns `not_leader`.
- **Upgrades:** in-place from any prior release. See
  [UPGRADING.md](UPGRADING.md).

## Not in this release (and not claimed)

- **Production multi-node clustering** — this is an experimental preview.
- **Automatic production failover** — leader election is real, but operational
  failover remains preview.
- **Dynamic membership** (join/leave) — membership is static.
- **Snapshot install over the wire** — answered as unsupported.
- **Linearizable follower reads** — followers reject reads by default.
- **Distributed transactions, sharding, and multi-region.**

See [ROADMAP.md](ROADMAP.md) for what is planned beyond the preview.
