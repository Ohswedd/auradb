# AuraDB v0.5.1 release notes

**Theme: Multi-node preview hardening.**

AuraDB v0.5.1 is a patch release that makes the v0.5.0 controlled multi-node
server preview safer, easier to operate, and more trustworthy. It changes no
on-disk format and no wire protocol (AWP 1), and it preserves Aura Connector
0.3.x compatibility — no connector release is required. **AuraDB v0.5.1 hardens
the controlled multi-node preview. Single-node mode remains the recommended
production mode.** Multi-node mode stays experimental, off by default, and gated
behind two explicit opt-ins.

This release makes **no production-clustering claims**. It does not implement
sharding, distributed transactions, multi-region, dynamic membership, production
automatic failover, or linearizable follower reads, and it does not claim any of
them.

## At a glance

- **Local Docker cluster with generated development certificates.** A new
  `examples/cluster/generate-dev-certs.sh` script (and a PowerShell companion)
  produces a local CA plus per-node certificates for `node1`, `node2`, and
  `node3` — each with Subject Alternative Names covering the node name,
  `localhost`, and `127.0.0.1` — under a git-ignored `examples/cluster/certs/`
  directory. `docker-compose.cluster.yml` consumes the generated certificates
  and a shared peer token, runs three nodes with persistent volumes, distinct
  client and cluster ports, and health checks, and `scripts/smoke_cluster_compose.sh`
  drives the whole flow (generate certs, start, wait for a leader, report status,
  tear down). Generated certificates are development-only and must never be used
  in production.
- **Multi-SAN development certificates.** `auradb cert generate-dev` now accepts
  `--server-name` and repeatable `--san` flags so each node gets a certificate
  whose SANs match how peers and clients address it. The previous no-argument
  form is unchanged.
- **Sharper cluster diagnostics.** `auradb cluster status --addr <server>` now
  queries a running server for live role, leader, term, quorum availability,
  commit/applied/last-log indices, replication lag, and per-peer reachability.
  `auradb cluster doctor` explains no-leader, no-quorum, unreachable-peer,
  follower-lag, and public-cluster-without-TLS conditions, and the cluster health
  section gained additive per-peer diagnostics fields.
- **More honest `not_leader` ergonomics.** A write routed to a follower returns
  `not_leader` with a stable error code, the current node id, the known leader id
  (and the leader's client address when an operator has declared it), and
  retry-vs-redirect guidance embedded in the human-readable message. The same
  client connection remains usable for status and health after a `not_leader`
  response, and an additive, optional `retryable` hint is carried in the error
  payload for clients that choose to read it.
- **Leader restart and re-election coverage.** New tests start three real nodes,
  stop the leader, confirm the surviving majority elects a new leader and keeps
  accepting writes, restart the old leader, and confirm it rejoins as a follower,
  catches up, and all nodes converge. This is **preview leader-restart behavior**,
  not production automatic failover.
- **Larger follower catch-up coverage.** New tests commit a long run of entries
  (including transaction batches and a compacted-log boundary) while a follower
  is down, then confirm the restarted follower replays its durable log and is
  brought current by the leader, with matching record counts across nodes. When
  catch-up would require installing a snapshot of compacted entries — which this
  preview does not implement — the condition surfaces as a structured,
  unsupported result rather than silent corruption or a hang.
- **Peer TLS rotation guidance and validation.** The docs describe how peer
  certificates are used, how to rotate them with a rolling restart, how to verify
  SANs, and how to rotate the peer token without committing secrets. Tests assert
  that a wrong CA, a wrong SAN, and a peer-token mismatch are rejected and that a
  node presenting a freshly rotated certificate is accepted after restart.
- **Replicated write latency baseline.** `benches/baseline/v0.5.1.json` records a
  same-machine baseline for regression tracking. Benchmarks are hardware-specific
  and preview cluster overhead is expected; the numbers are not a universal
  performance claim.
- **Stronger cluster CI.** The cluster workflow runs the deterministic Raft tests
  and the process-level loopback cluster tests as required jobs and validates the
  Docker Compose configuration; the live Docker Compose smoke is available as a
  manual / nightly job to avoid a flaky required check.

## Compatibility

- **On-disk format unchanged.** Storage v2, cluster metadata v1, Raft log/state,
  compaction marker, commit base, and the snapshot manifest are all unchanged. A
  v0.5.0 data directory opens directly; a v0.5.1 directory can be reopened by
  v0.5.0.
- **Wire protocol unchanged.** AWP 1. The new cluster diagnostics fields and the
  optional `retryable` error hint are additive and ignored by older clients.
- **Connector.** Aura Connector 0.3.x remains fully compatible. No connector
  release is required.

## What this release is not

Single-node mode remains the recommended production mode. The multi-node path is
an experimental, opt-in preview with static membership and no fault-tolerance
guarantees for a single-node cluster. Production multi-node clustering, automatic
failover, dynamic membership, linearizable or follower reads, distributed
transactions, sharding, and multi-region are not implemented and not claimed.
