# Operator runbooks

Practical, copy-pasteable procedures for running AuraDB **v0.8.0** in
single-node production-candidate mode, plus the experimental cluster preview.
Each runbook lists symptoms, the commands to run, safe actions, actions to avoid,
expected output, and when to restore from backup.

> AuraDB v0.8.0 is a production-readiness candidate for **single-node**
> deployments and a stronger cluster **preview**. It is **not** production HA.
> Single-node mode remains the recommended production mode. See
> [PRODUCTION_READINESS.md](PRODUCTION_READINESS.md).

Conventions: `DATA=/var/lib/auradb` is the data directory; `ADDR=127.0.0.1:7171`
is a running server's client address. Replace as appropriate.

---

## 1. Pre-production checklist

Work through [PRODUCTION_READINESS.md](PRODUCTION_READINESS.md) in full. At a
minimum: auth + TLS enabled, scheduled and *rehearsed* backups, monitoring wired,
the upgrade path tested in staging, and a rollback plan.

## 2. First deployment

- **Commands**
  ```bash
  auradb init --data-dir "$DATA" --config /etc/auradb/AuraDB.toml
  auradb auth hash-token            # paste into [auth].token_hash, set enabled = true
  auradb config validate --config /etc/auradb/AuraDB.toml
  auradb server --config /etc/auradb/AuraDB.toml
  auradb status --addr "$ADDR" --json
  ```
- **Expected**: `config validate` prints "configuration is valid"; `status`
  reports `"status":"healthy","ready":true`.
- **Safe**: run as a non-root user; keep the config readable only by that user.
- **Avoid**: binding a public interface with auth disabled (refused unless
  `allow_insecure_bind`).

## 3. Secure configuration

- Enable `[auth]` (token hash from `auradb auth hash-token`) and `[tls]` (cert +
  key; add `require_client_cert` for mTLS).
- **Verify**: `auradb config validate` and confirm `auradb doctor --json` shows
  auth/TLS enabled with secrets redacted.
- **Avoid**: storing plaintext tokens or private keys in images or version
  control. See [SECURITY.md](SECURITY.md).

## 4. Backup and restore

- **Backup**
  ```bash
  auradb dump --data-dir "$DATA" --out backup-$(date +%F).jsonl
  auradb backup verify --input backup-$(date +%F).jsonl --json
  ```
- **Restore (rehearsal or recovery)**
  ```bash
  auradb restore --data-dir /tmp/restore-test --input backup-YYYY-MM-DD.jsonl
  auradb check --data-dir /tmp/restore-test --json
  ```
- **Expected**: `backup verify` reports `"ok": true`; `check` reports
  `"ok": true` with the expected record count.
- **What `backup verify` rejects**: a malformed or truncated line, an invalid
  schema section, a record for a collection no schema declares, a line past the
  per-line size bound, and a backup that carries two records with the same primary
  key (a corrupt or hand-edited dump whose restore would silently collapse two
  logical records into one). The report names the collection and a count only —
  it never prints field values or key contents.
- **Required before trusting a backup**: run the restore rehearsal above (restore
  into a fresh directory and `check`) — a backup you have never restored is not a
  backup you can trust.
- **Safe**: always restore into a *fresh* directory.
- **Avoid**: restoring over a live data directory while the server is running.

## 5. Upgrade

- **Commands**
  ```bash
  auradb dump --data-dir "$DATA" --out pre-upgrade.jsonl   # 1. backup
  auradb backup verify --input pre-upgrade.jsonl --json
  # 2. stop server, swap the binary, 3. then:
  auradb check --data-dir "$DATA" --json                   # 4. pre-flight
  auradb server --config /etc/auradb/AuraDB.toml           # 5. start
  auradb check --data-dir "$DATA" --json                   # 6. post-flight
  ```
- **Rollback**: stop, restore the previous binary, and if needed
  `auradb restore` the pre-upgrade backup into a fresh directory.
- See [UPGRADING.md](UPGRADING.md). **Restore from backup** if `check` reports
  `ok == false` after the upgrade and `index rebuild` does not clear it.

## 6. Storage check failure

- **Symptoms**: `auradb check --json` reports `"ok": false`; server fails to open.
- **Commands**
  ```bash
  auradb check --data-dir "$DATA" --json
  ```
- **Triage by the failing section**:
  - `storage` (manifest/segment/format): corruption or an unknown future format.
    Do **not** edit files by hand. **Restore from backup.**
  - `catalog`: schema catalog corrupt. **Restore from backup.**
  - `indexes` rebuilt > 0 (a warning, `ok` still true): run
    `auradb index rebuild` to persist fresh snapshots.
  - `planner_stats` (warning): run `auradb stats analyze`.
  - `raft` / `snapshots` (cluster): see runbooks 14–18.
- **Avoid**: hand-editing `MANIFEST`, `catalog.json`, or segment files.

## 7. Index corruption / rebuild

- **Symptoms**: `check` shows `indexes.consistency_ok = false` or `rebuilt > 0`.
- **Commands**
  ```bash
  auradb index check --data-dir "$DATA"
  auradb index rebuild --data-dir "$DATA"
  auradb check --data-dir "$DATA" --json
  ```
- **Safe**: index rebuild is non-destructive (indexes are derived from storage).
- **Expected**: after rebuild, `consistency_ok = true`.

## 8. Planner statistics rebuild

- **Symptoms**: `doctor`/`check` warns that statistics look stale or unreadable;
  query plans look poor.
- **Commands**
  ```bash
  auradb stats analyze --data-dir "$DATA"
  auradb stats show --data-dir "$DATA" --json
  ```
- **Safe**: statistics are advisory; rebuilding never affects data.

## 9. Disk pressure

- **Symptoms**: data directory growing; disk-usage alert.
- **Commands**
  ```bash
  auradb gc --data-dir "$DATA" --dry-run     # preview reclaimable versions
  auradb gc --data-dir "$DATA"               # reclaim
  auradb compact --data-dir "$DATA"          # rewrite live segments
  auradb check --data-dir "$DATA" --json
  ```
- **Avoid**: compacting on a volume with no headroom — compaction writes new
  segments before removing old ones. Ensure free space first.

## 10. Long-running transaction

- **Symptoms**: `doctor`/`status` warns of old active snapshots; retained
  versions climbing; GC not reclaiming.
- **Commands**
  ```bash
  auradb doctor --data-dir "$DATA" --json    # oldest_transaction_age_secs, active
  ```
- **Safe**: ensure clients commit/rollback promptly; set
  `mvcc.transaction_timeout_secs` so abandoned transactions are reaped.
- **Avoid**: disabling transaction timeouts in production.

## 11. GC pressure

- **Symptoms**: retained versions far exceed live records.
- **Commands**: as in runbook 9 (`gc --dry-run`, then `gc`). Confirm
  `mvcc.gc_enabled = true` and a sane `gc_interval_secs`.

## 12. Full-text / index issue

- **Symptoms**: full-text or document-path queries return unexpected results.
- **Commands**
  ```bash
  auradb index check --data-dir "$DATA"
  auradb index rebuild --data-dir "$DATA"
  ```
- **Note**: full-text is tokenized matching, not BM25 ranking (by design).

## 13. Snapshot inspect / restore

- **Commands**
  ```bash
  auradb snapshot inspect --input snapshot.bin
  auradb snapshot restore --input snapshot.bin --data-dir /tmp/restore --force
  auradb check --data-dir /tmp/restore --json
  ```
- **Expected**: `inspect` confirms the digest verifies. Restore into a fresh dir.

---

### Cluster preview (experimental — not production HA)

## 14. Cluster preview leader loss

- **Symptoms**: writes return `not_leader`; `cluster status` shows no leader.
- **Commands**
  ```bash
  auradb cluster status --addr "$ADDR" --json
  auradb cluster wait-leader --addr "$ADDR" --timeout-secs 30
  auradb cluster doctor --addr "$ADDR" --json
  ```
- **Expected**: with a majority alive, a new leader is elected within seconds.
- **Avoid**: assuming automatic failover guarantees — this is a preview.

## 15. Follower lag

- **Symptoms**: `cluster status` reports a follower behind by many entries.
- **Commands**: `auradb cluster status --addr "$ADDR" --json` (per-peer state).
- **Safe**: a lagging follower catches up via append-entries or a snapshot
  install. Give it time and bandwidth.

## 16. Snapshot needed

- **Symptoms**: a follower is too far behind for log catch-up; `status`/`doctor`
  reports a snapshot is needed.
- **Commands**
  ```bash
  auradb cluster compact-log --data-dir "$DATA" --dry-run
  auradb cluster compact-log --data-dir "$DATA"
  ```
- The leader installs a snapshot to the follower automatically when required.

## 17. Peer TLS failure

- **Symptoms**: peers cannot connect; logs show TLS handshake errors.
- **Triage**: verify peer cert/key/CA paths and validity; regenerate dev certs
  with `examples/cluster/generate-dev-certs.sh`. See
  [CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md).

## 18. Peer token mismatch

- **Symptoms**: peers reject each other with an auth error.
- **Triage**: ensure every node shares the same `peer_auth_token`. Rotate the
  token on all nodes together (see [SECURITY.md](SECURITY.md)).

---

### HA release-candidate recovery runbooks (v0.9.0, refined in v0.9.1)

> These cover the controlled static-cluster preview. It is an **HA release
> candidate, not a production HA guarantee** (v0.9.1 stabilizes it); single-node
> mode remains the recommended production mode. See
> [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) for the support level, the
> operator assumptions, and the validated failure matrix. Every runbook below
> lists when to restore from backup and what to include in a bug report.

For every runbook: **bug report** = `auradb version`, the redacted
`auradb cluster status --addr "$ADDR" --json` and `auradb cluster doctor --addr
"$ADDR" --json` from each reachable node, the node logs around the event, and
the minimal reproduction. AuraDB redacts secrets in those reports.

#### 18a. Leader process killed

- **Symptoms**: writes to the old leader fail; `cluster status` on a survivor
  shows a re-election.
- **Commands**: `auradb cluster wait-leader --addr "$SURVIVOR" --timeout-secs 30`;
  `auradb cluster leader --addr "$SURVIVOR"`.
- **Expected**: with a majority alive, a new leader is elected within seconds and
  writes resume on it.
- **Safe**: let the supervisor restart the old node; it rejoins as a follower.
- **Unsafe**: forcing a second node down (you may lose quorum).
- **Restore from backup**: not needed for a single leader loss with quorum
  intact.

#### 18b. Leader graceful shutdown

- **Symptoms**: a planned stop of the leader; the cluster re-elects.
- **Commands**: stop the process via your supervisor; then
  `auradb cluster wait-leader --addr "$SURVIVOR" --timeout-secs 30`.
- **Expected**: identical to a kill from the cluster's view (there is no
  `step-down`; stopping the process is the supported path).
- **Unsafe**: stopping a second node before the first has rejoined.
- **Restore from backup**: not needed.

#### 18c. No leader

- **Symptoms**: every node reports no leader; writes return `not_leader`.
- **Commands**: `auradb cluster status --addr "$ADDR" --json` on each node;
  `auradb cluster doctor --addr "$ADDR" --json`; check `quorum_available`.
- **Expected**: if a majority is alive, an election completes; if not, see
  *Quorum lost*.
- **Safe**: confirm peer connectivity (TLS, token, network) — a peer auth/TLS
  fault can stall elections.
- **Restore from backup**: only if storage corruption is also indicated by
  `auradb check`.

#### 18d. Quorum lost

- **Symptoms**: a minority is alive; `quorum_available` is `false`; no writes
  commit.
- **Commands**: `auradb cluster status --addr "$ADDR" --json` (count live peers).
- **Expected**: the minority **cannot** and **must not** commit (this is the
  safety property). Restore the majority by bringing stopped nodes back.
- **Unsafe**: never force a minority to accept writes; that risks split-brain and
  data loss.
- **Restore from backup**: if the majority is unrecoverable, restore the latest
  leader backup to a single node and rebuild the cluster (see 18m).

#### 18e. Old leader rejoins

- **Symptoms**: a previously-stopped leader restarts.
- **Commands**: `auradb cluster wait-ready --addr "$REJOINED" --timeout-secs 60`;
  `auradb cluster status --addr "$LEADER" --json` (per-peer `match_index`).
- **Expected**: it rejoins as a follower at the current term and catches up by
  log replay or a snapshot install.
- **Restore from backup**: not needed.

#### 18f. Follower stuck behind

- **Symptoms**: a follower's `match_index` is far behind and not advancing.
- **Commands**: `auradb cluster doctor --addr "$LEADER" --json` (follower lag,
  snapshot-needed); check disk and network on the follower.
- **Expected**: it catches up via append-entries, or a snapshot install if it
  fell behind the compacted prefix.
- **Safe**: give it bandwidth; verify its disk is not full.
- **Restore from backup**: only if `auradb check` on the follower reports
  storage corruption.

#### 18g. Snapshot needed / snapshot install failing

- **Symptoms**: `doctor` reports a snapshot is needed; or snapshot counters show
  a rejected install.
- **Commands**: `auradb cluster status --addr "$ADDR" --json` (snapshot
  counters: sent / installed / rejected); node logs for the rejection reason.
- **Expected**: the leader installs a snapshot automatically. A rejected install
  (oversized, wrong cluster, bad digest, future format) is **safe** — existing
  follower state is preserved and the install is retried.
- **Unsafe**: hand-editing a follower's data dir.
- **Restore from backup**: if a follower's local state is corrupt, stop it,
  restore the latest leader backup to a fresh single node, and re-add it offline.

#### 18h. Minority / majority partition

- **Symptoms**: a network split; one side has quorum, the other does not.
- **Commands**: `auradb cluster status --addr "$ADDR" --json` on each side
  (`quorum_available`).
- **Expected**: the majority side keeps committing; the minority serves no
  writes and rejoins on heal.
- **Unsafe**: never run two majorities (do not reconfigure membership during a
  partition).
- **Restore from backup**: not needed; heal the network.

#### 18i. Peer reconnect storm

- **Symptoms**: repeated peer connect/disconnect churn; `doctor` warns on a
  reconnect storm.
- **Commands**: `auradb cluster doctor --addr "$ADDR" --json`; check the
  network and the peers' clocks/load.
- **Expected**: bounded-backoff reconnects recover replication without duplicate
  apply.
- **Restore from backup**: not needed.

#### 18j. Peer TLS failure / token mismatch

- See **17. Peer TLS failure** and **18. Peer token mismatch** above. Rotate
  certs/tokens on **all** nodes together; validate with
  `auradb config validate --config <node>.toml`. Restore from backup is not
  required for a transport-auth fault.

#### 18k. Published-image HA smoke failed

- **Symptoms**: `scripts/smoke_ha_candidate.sh` exits non-zero.
- **Commands**: re-run with the failing image; read the dumped `docker compose
  logs`; confirm the image tag/version matches (the script fails loudly on a
  mismatch).
- **Expected**: the smoke is a candidate check, not production HA proof. Treat a
  failure as a release blocker for the cluster preview, not a single-node
  blocker.
- **Restore from backup**: N/A (smoke uses throwaway volumes).

#### 18l. Roll back from a bad release

- **Symptoms**: a new AuraDB version misbehaves in the preview cluster.
- **Commands**: stop all nodes; redeploy the previous image tag; start nodes;
  `auradb cluster wait-leader`. The storage format (v2) is unchanged, so a
  same-format rollback needs no migration.
- **Safe**: take a backup from the current leader before rolling back.
- **Unsafe**: rolling back across a storage-format change (none in this release).
- **Restore from backup**: if the bad release wrote unexpected data, restore the
  pre-upgrade backup to a single node (see 18m).

#### 18m. Restore a single-node backup after a cluster incident

- **When**: the majority is unrecoverable, or corruption is confirmed by
  `auradb check`.
- **Commands**
  ```bash
  # 1. Take/locate the latest backup from the (most current) leader.
  auradb dump --data-dir "$LEADER_DATA" --out latest.jsonl
  auradb backup verify --input latest.jsonl
  # 2. Restore into a FRESH single-node data dir (never a live cluster).
  auradb restore --data-dir /var/lib/auradb-restored --input latest.jsonl
  auradb check --data-dir /var/lib/auradb-restored --json
  # 3. Run single node in production, or bootstrap a fresh preview cluster
  #    around the restored data dir (see CLUSTERING.md).
  ```
- **Expected**: the restored single node carries the latest committed state.
- **Unsafe**: restoring into a running multi-node cluster (unsupported; restore
  targets an offline, fresh data dir).

#### 18n. `not_leader` without a leader client address — re-resolve the leader (v0.9.1)

- **Symptoms**: a write returns `not_leader` but the response carries no
  `leader_client_addr` (the client cannot redirect from the hint alone).
- **Why**: the leader's client address is reported only when declared — a peer's
  `client_addr`, or the leader's own `[cluster] advertise_client_addr`. Undeclared
  ⇒ reported as unknown (never guessed). Expected too in Docker Compose, where the
  in-network hint (e.g. `node2:7171`) is not the host-published port.
- **Commands** (the documented fallback)
  ```bash
  # Ask any reachable node who leads (and its client address, if known):
  auradb cluster leader --addr "$ADDR" --json
  # Or read the full cluster view, including leader_client_addr:
  auradb cluster status --addr "$ADDR" --json
  # If still unknown, probe each candidate client address — the leader accepts a write:
  for a in "$NODE1" "$NODE2" "$NODE3"; do auradb cluster leader --addr "$a" --json; done
  ```
- **Stale hint vs. no leader**: a hint naming a leader id but no client address
  means re-resolve; "no leader is currently known" means an election is in
  progress — `auradb cluster wait-leader` and retry, do not treat as a failure.
- **Make it reliable**: set each node's `advertise_client_addr` to its own client
  address (matching peers' `client_addr` for it); then the leader self-reports its
  hint after a leader change. See [CONFIGURATION.md](CONFIGURATION.md) and
  [CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md).
- **Logs to collect**: `auradb version`; redacted `cluster status`/`cluster
  doctor` JSON from each node; node logs around the leader change.
- **Restore from backup**: not needed — this is a routing/redirect concern, not
  data loss.

#### 18o. Final operator clarity & v1.0 evidence collection (v0.9.2)

A consolidated checklist for the leader-hint / client-address questions and for
collecting HA-candidate evidence toward a v1.0 readiness report. v0.9.2 adds no new
config or behavior; this gathers the existing guidance in one place.

- **Leader hint missing** → re-resolve the leader (runbook 18n). Not a bug; not
  data loss.
- **Leader hint unreachable** (a declared `leader_client_addr` that does not
  accept connections) → the client must fall back to re-resolve/probe (18n). This
  is the documented behavior in Docker Compose, where the in-network hint (e.g.
  `node2:7171`) is **not** the host-published port (e.g. `127.0.0.1:7181`).
- **Docker in-network vs. host-published addresses** → a client *inside* the
  Docker network uses the in-network hint directly; a client on the **host**
  reaches nodes on the published ports and re-resolves. See
  [CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md).
- **When to use `advertise_client_addr`** → set it on every node to its own client
  address (matching the `client_addr` peers list for it) so a leader self-reports
  its hint after a leader change.
- **Rotating `advertise_client_addr` safely** → it is operator-declared and
  additive: update one node's config and restart that node (the supervisor brings
  it back as a follower; it self-reports the new address only while it leads).
  Update peers' `client_addr` for that node to match. No storage or wire change is
  involved; do it node-by-node to avoid a window where the value disagrees.
- **Routing issue vs. no-leader issue** → a `not_leader` naming a leader id but no
  client address is a **routing** issue (re-resolve, 18n). "No leader currently
  known" / `quorum_available: false` is a **no-leader** issue (wait for an
  election or restore quorum — runbooks 14, 18b, 18c).
- **Run both published-image smokes** → after a release,
  `AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.9.2 bash scripts/smoke_cluster_compose.sh`
  and `… bash scripts/smoke_ha_candidate.sh` (see [RELEASE.md](RELEASE.md)). Use
  `KEEP_ARTIFACTS=1` to retain logs/certs.
- **Collect evidence for a v1.0 readiness report** → record image digest, per-node
  server versions, leader before/after a kill, the leader client-address source
  (advertised / status / fallback / probe), connector version, and the serial
  multi-node suite result. Map each against
  [V1_0_DECISION_CHECKLIST.md](V1_0_DECISION_CHECKLIST.md) §5 (evidence that
  exists) and §6 (evidence still missing).
- **Restore is for data recovery, not routing** → a leader-hint / redirect problem
  is never fixed by restoring a backup; restore only for storage/catalog/metadata
  corruption (runbook 19).
- **When to stay on single-node production mode** → unless every production-HA
  criterion in [HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) §8 /
  [V1_0_DECISION_CHECKLIST.md](V1_0_DECISION_CHECKLIST.md) §4 is met and
  documented, run **single-node** in production and treat multi-node as a
  controlled static-cluster preview.

---

## 19. Restoring from backup

When in doubt, restore. A logical restore is the safe recovery path for storage
or catalog corruption:

```bash
auradb restore --data-dir /var/lib/auradb-new --input last-good-backup.jsonl
auradb check --data-dir /var/lib/auradb-new --json
# point the server's data_dir at the restored directory and start it
```

Restore from backup when: `check` reports a `storage` or `catalog` error;
`index rebuild` does not clear an inconsistency; or a disk/hardware fault is
suspected.

## 20. Reporting a bug

Collect, with secrets redacted (AuraDB redacts secrets in `doctor`/`status`/`check`):

```bash
auradb version
auradb doctor --data-dir "$DATA" --json
auradb check --data-dir "$DATA" --json
```

Include the AuraDB version, the redacted reports, what you expected, what
happened, and the minimal steps to reproduce. See `SECURITY.md` for reporting
security issues privately.
