# AuraDB multi-node preview cluster (experimental)

> **Preview, not production.** This is the first controlled preview of real
> cross-process multi-node AuraDB. It is intended for local testing and early
> validation only. **Single-node mode remains the recommended production path.**
> See [docs/CLUSTERING.md](../../docs/CLUSTERING.md).

Forming a real cluster requires two explicit opt-ins in `[cluster]`:

```toml
enabled = true
experimental_multi_node = true
```

Membership is **static** — every node lists every other node by `node_id` and
`addr`. There is no join, leave, or dynamic membership. Writes go to the leader;
followers reject writes with `not_leader` and a leader hint.

---

## Option A — three local processes (loopback, no TLS)

This is the simplest, fully-loopback way to try the preview. All traffic stays on
`127.0.0.1`, so no peer TLS or token is required.

Open three terminals from the repository root:

```bash
auradb server --config examples/cluster/node1.toml
auradb server --config examples/cluster/node2.toml
auradb server --config examples/cluster/node3.toml
```

Client ports are `7171`, `7181`, `7191`; cluster (Raft) ports are `7172`, `7182`,
`7192`.

### Inspect the cluster

```bash
# Wait for an election, then see who won.
auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30
auradb cluster leader      --addr 127.0.0.1:7171
auradb cluster status      --addr 127.0.0.1:7171 --json   # includes per-peer state
```

### Write through the leader, observe a follower

```bash
# Find the leader's client address from `cluster leader`/`status`, then use the
# Aura Connector (or any AWP client) against it. A write sent to a follower
# returns a structured `not_leader` error with a leader hint.
```

### Stop a follower, keep writing, restart, watch catch-up

```bash
# 1. Stop one follower (Ctrl-C in its terminal).
# 2. Keep writing through the leader (a 2/3 majority remains).
# 3. Restart the follower with the same config:
auradb server --config examples/cluster/node2.toml
# 4. It replays its durable log and the leader brings it current. Confirm:
auradb cluster status --addr 127.0.0.1:7181 --json
```

### Shut down

Stop each process (Ctrl-C). Data persists under `.local/cluster/node{1,2,3}`.

---

## Option B — Docker Compose (Docker bridge network, TLS + token)

Because containers communicate over a non-loopback Docker network, this path is a
"public" cluster and requires peer TLS plus a shared peer authentication token
(the configs under [`docker/`](docker) already set the required flags).

1. **Generate peer certificates** whose SANs cover the service names
   `node1`, `node2`, `node3` into `./examples/cluster/certs` as `peer.crt`,
   `peer.key`, and `ca.crt`. Any standard tooling works; the certificate must be
   trusted by `ca.crt` and present a SAN for each peer hostname.

2. **Choose a shared token** and set it identically as `peer_auth_token` in each
   of `examples/cluster/docker/node{1,2,3}.toml`, replacing
   `change-me-preview-cluster-token`.

3. **Validate, then start:**

   ```bash
   docker compose -f docker-compose.cluster.yml config     # structure check
   docker compose -f docker-compose.cluster.yml up -d
   ```

4. **Inspect** (client ports are published to the host as `7171`/`7181`/`7191`):

   ```bash
   auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30
   auradb cluster status      --addr 127.0.0.1:7171 --json
   ```

5. **Stop / restart a follower** to watch catch-up:

   ```bash
   docker compose -f docker-compose.cluster.yml stop node2
   docker compose -f docker-compose.cluster.yml start node2
   ```

6. **Tear down** (removing volumes):

   ```bash
   docker compose -f docker-compose.cluster.yml down -v
   ```

The loopback option (A) is the validated path used by the project's integration
tests; the Docker option (B) is provided for a more realistic networked preview.

---

## What is and is not in the preview

**Real and tested:** leader election across processes, replicated writes with
majority commit, follower apply and catch-up after restart, leader-only writes
with `not_leader` routing, and cluster status/diagnostics across peers.

**Not in the preview:** dynamic membership (join/leave), snapshot install over the
wire (answered as unsupported), automatic production failover guarantees,
linearizable follower reads, distributed transactions, sharding, and multi-region.
