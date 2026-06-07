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
