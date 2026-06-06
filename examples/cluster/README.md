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
# Live cluster diagnostics: role, leader (and its client address), quorum,
# commit/applied/last-log indices, replication lag, and per-peer reachability.
auradb cluster status --addr 127.0.0.1:7171 --json
auradb status        --addr 127.0.0.1:7171 --json   # general health + cluster
```

Or run the whole loopback flow end to end:

```bash
bash scripts/smoke_cluster_loopback.sh
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
auradb status --addr 127.0.0.1:7181 --json
```

### Shut down

Stop each process (Ctrl-C). Data persists under `.local/cluster/node{1,2,3}`.

---

## Option B — Docker Compose (Docker bridge network, TLS + token)

Because containers communicate over a non-loopback Docker network, this path is a
"public" cluster and requires peer TLS plus a shared peer authentication token
(the configs under [`docker/`](docker) already set the required flags).

1. **Generate development peer certificates.** This produces a shared local CA
   (`ca.crt`/`ca.key`) and per-node certificates (`node1.crt`/`node1.key`, …),
   each with SANs covering its service name plus `localhost` and `127.0.0.1`,
   under the git-ignored `examples/cluster/certs/`:

   ```bash
   bash examples/cluster/generate-dev-certs.sh   # or generate-dev-certs.ps1 on Windows
   ```

   > **Development only.** The generated CA and keys are self-signed and
   > unencrypted. Never use them in production. A peer dialing `node2:7172`
   > verifies the certificate's SAN against `node2`, which is why each node has
   > its own certificate.

2. **Choose a shared token** and set it identically as `peer_auth_token` in each
   of `examples/cluster/docker/node{1,2,3}.toml`, replacing
   `change-me-preview-cluster-token`.

3. **Validate, then start:**

   ```bash
   docker compose -f docker-compose.cluster.yml config     # structure check
   docker compose -f docker-compose.cluster.yml up -d
   ```

   Or do steps 1–4 in one shot (generate certs, start, wait for a leader, report
   status, tear down). Select the image with `AURADB_IMAGE` — a locally built
   image (the recommended preview path, no registry pull) or the published one:

   ```bash
   # Local image (build it first: docker build -t auradb:0.6.1 .)
   AURADB_IMAGE=auradb:0.6.1 bash scripts/smoke_cluster_compose.sh
   # Or the published image (post-release verification)
   AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.6.1 bash scripts/smoke_cluster_compose.sh
   ```

   The smoke prints the image used, the node ports, the leader, quorum, per-peer
   states, and the teardown result. The published image is **multi-arch**
   (`linux/amd64` + `linux/arm64`), so `docker pull` selects arm64 automatically
   on Apple Silicon; inspect the manifest with `docker buildx imagetools inspect
   ghcr.io/ohswedd/auradb:0.6.1`.

4. **Inspect** (client ports are published to the host as `7171`/`7181`/`7191`):

   ```bash
   auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 60
   auradb cluster status --addr 127.0.0.1:7171 --json
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

## Rotating peer certificates and the peer token

Peer certificates and the shared peer token are long-lived secrets. Rotate them
with a **rolling restart**, one node at a time, so a quorum stays available.

**Certificates (same CA).** Re-issue a node's certificate from the same CA
(`generate-dev-certs.sh` reuses the existing `ca.crt`/`ca.key` in the directory),
then restart just that node. Because peers trust the CA — not a specific leaf —
the rotated certificate is accepted without touching the others. A node that
presents a certificate from the **wrong CA**, or whose **SAN does not match** the
dialed peer name, is rejected by the TLS handshake (validated by the
`peer_tls` tests). Repeat per node.

**Rotating the CA.** Issue a new CA, distribute a bundle containing **both** the
old and new CA in each node's `ca_path`, roll every node to certificates signed by
the new CA, then drop the old CA from the bundle in a second roll.

**Peer token.** Set the new `peer_auth_token` on each node and restart it; during
the roll, nodes still on the old token fail the handshake (`AuthFailed`) until
they are updated, so roll quickly and watch `auradb cluster status --addr` for
peer connectivity. The token is redacted in logs and `Debug` output.

Never commit `certs/` or any `.key`/token to version control — the example
directory's `.gitignore` excludes them.

---

## What is and is not in the preview

**Real and tested:** leader election across processes, replicated writes with
majority commit, follower apply and catch-up after restart, leader-only writes
with `not_leader` routing, and cluster status/diagnostics across peers.

**Not in the preview:** dynamic membership (join/leave), snapshot install over the
wire (answered as unsupported), automatic production failover guarantees,
linearizable follower reads, distributed transactions, sharding, and multi-region.
