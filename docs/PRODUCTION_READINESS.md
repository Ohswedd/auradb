# Production readiness

AuraDB **v1.0.1** is the **first production patch on the v1.0 single-node
production line**, with a multi-node HA candidate preview. It supports production
single-node deployments configured with authentication, TLS, backups, monitoring,
and the documented runbooks. It is **not** production HA. **Single-node mode is the
recommended production mode.**

The production-support statement is **scoped**, not blanket: single-node mode, run
with the checklist below (authentication, TLS, scheduled backups with a rehearsed
restore, and monitoring), is the supported production path. A single-node
deployment that omits auth, TLS, backups, or monitoring is not a supported
production configuration. The multi-node cluster is an **HA candidate preview** — a
controlled static-cluster preview with strong release-candidate evidence, validated
against a failure matrix — and is **not** production HA: no production automatic
failover, no linearizable follower reads, no distributed transactions, no dynamic
membership, no sharding, no multi-region. v1.0.1 carries forward all v1.0.0
behavior and changes none of this scope; it re-verifies the v1.0 production gates
on the v1.0.1 build and keeps the AWP 1 and storage format v2 compatibility
surfaces, the upgrade guarantee, the backup/restore release gate, and the security
review unchanged.

The authoritative support boundary is [SUPPORT_POLICY.md](SUPPORT_POLICY.md). The
exact support level for each mode, the operator assumptions the static cluster
requires, the validated failure matrix, and the **strict criteria that must be met
and documented before AuraDB ever claims production HA** are in
[HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md) and the
[v1.0 decision checklist](V1_0_DECISION_CHECKLIST.md). None of those production-HA
criteria are met in v1.0.1; multi-node remains an HA candidate preview.

**Frozen for v1.** AuraDB v1.0.1 uses Aura Wire Protocol 1 and storage format v2.
AWP 1 is the stable v1 wire protocol and storage format v2 is the stable v1
single-node storage format; AuraDB v1.x preserves both unless a security,
correctness, safety, or corruption issue requires a documented change or migration.
See [COMPATIBILITY.md](COMPATIBILITY.md) and [STORAGE_ENGINE.md](STORAGE_ENGINE.md).

This document states, honestly, what is supported at what level, and gives the
checklists to run before and during a production single-node deployment. For
step-by-step procedures see [RUNBOOKS.md](RUNBOOKS.md).

## Support levels

### Production-supported

- **Single-node mode** with authentication and TLS enabled, scheduled backups,
  monitoring, and the upgrade runbook. This is the recommended way to run AuraDB
  in production, and the supported production deployment mode in v1.0.

### HA candidate preview (not for production)

- **Multi-node static cluster mode.** Real cross-process leader election and Raft
  replication work and are tested, but the preview is off by default, gated
  behind two explicit `[cluster]` opt-ins, uses static membership, and makes no
  high-availability guarantee. It is an HA candidate preview with strong
  release-candidate evidence, **not** production HA. Use it to evaluate the
  direction, not to serve production traffic that requires high availability.

### Not supported (do not rely on these)

- production HA / automatic failover guarantees;
- dynamic cluster membership;
- distributed transactions;
- sharding;
- multi-region replication;
- linearizable follower reads (followers reject client reads);
- approximate vector search (ANN / HNSW);
- BM25 / hybrid lexical-vector fusion.

## Known limitations (by design)

- **Snapshot isolation, not serializable.** Transactions read from a snapshot
  pinned at `begin` with optimistic write-conflict detection. This is single-node
  snapshot isolation, not serializable isolation.
- **Exact vector search.** Nearest-neighbour search is exact (brute-force over the
  collection), not approximate. It is correct but scales linearly.
- **Tokenized full-text, not BM25.** Full-text search matches on tokens; there is
  no BM25 ranking or hybrid lexical-vector fusion.
- **Cluster preview caveats.** See the experimental-preview note above and
  [CLUSTERING.md](CLUSTERING.md).

## Production single-node release runbook checklist

Run this before declaring a single-node deployment production-ready. Each item
expands into the detailed sections below and the step-by-step procedures in
[RUNBOOKS.md](RUNBOOKS.md).

1. Configure authentication (`[auth] enabled = true`, Argon2id token hash).
2. Configure TLS (`[tls] enabled = true`, valid cert/key; consider mutual TLS).
3. Configure scheduled backups (`auradb dump` + `auradb backup verify`).
4. Run a restore drill (`auradb restore` into a scratch dir + `auradb check`).
5. Configure monitoring (health endpoint + Prometheus metrics + disk alerting).
6. Configure log retention.
7. Run `auradb check --json` and confirm `ok == true`.
8. Run an upgrade rehearsal (backup → swap binary in a canary → `check`).
9. Verify disk capacity and headroom for the data directory.
10. Document the rollback plan (keep the previous binary and pre-upgrade backup;
    rollback means restore from backup).

## Backup and restore release gate

A v1.0 release must pass this backup/restore gate, exercised by the backup/restore
and upgrade-gate tests in `auradb-cli` (see [TESTING.md](TESTING.md) and
[RELEASE.md](RELEASE.md)):

1. Back up a mixed dataset (indexes, stats, relationships, vectors, full-text,
   document-path) with `auradb dump`.
2. Validate the dump without importing it: `auradb backup verify --input <file> --json`.
3. Restore into a fresh single-node directory: `auradb restore`.
4. Run `auradb check --json` on the restored directory.
5. Query smoke.
6. Relationship-include smoke.
7. Vector smoke.
8. Full-text smoke.
9. Document-path smoke.
10. Index and planner-stats validation.
11. Confirm no secrets appear in the backup-verify output.

```bash
auradb dump --data-dir ./data --output backup.jsonl
auradb backup verify --input backup.jsonl --json
auradb restore --data-dir ./restore --input backup.jsonl
auradb check --data-dir ./restore --json
```

## Single-node production checklist

### 1. Secure configuration

- [ ] Authentication enabled (`[auth] enabled = true`) with a token hash
      generated by `auradb auth hash-token`.
- [ ] TLS enabled (`[tls] enabled = true`) with a valid certificate and key;
      consider mutual TLS (`require_client_cert = true`) for service-to-service.
- [ ] Container runs as a non-root user (the published image already does).
- [ ] Secrets (token hash, TLS key) externalized via your secret manager, not
      baked into images or committed to version control.
- [ ] No public bind without auth/TLS. A non-loopback bind with auth disabled is
      refused at startup unless `allow_insecure_bind` is explicitly set
      (development only).
- [ ] If running the cluster preview: a peer auth token and peer TLS material are
      configured (loopback is the default; a public peer transport requires both).
- [ ] Defensive `[limits]` reviewed for your workload (see
      [CONFIGURATION.md](CONFIGURATION.md)); the defaults are safe.

### 2. Backup

- [ ] Scheduled logical dump (`auradb dump`) on a fixed cadence.
- [ ] Each backup validated with `auradb backup verify --input <file> --json`.
- [ ] Restore rehearsed into a scratch directory and checked with
      `auradb check --json` (a backup you have never restored is not a backup).
- [ ] Backup retention and off-host storage defined.
- [ ] Encryption at rest for backup files recommended (AuraDB does not encrypt
      dumps; use your storage layer or an external tool).

### 3. Upgrade

- [ ] Read the release notes (e.g. [V0_8_RELEASE_NOTES.md](V0_8_RELEASE_NOTES.md))
      and [UPGRADING.md](UPGRADING.md).
- [ ] Take a backup first.
- [ ] Run `auradb check --json` before and after.
- [ ] Stage the upgrade (test environment / canary) before production.
- [ ] Have a rollback plan: keep the previous binary and the pre-upgrade backup.

### 4. Monitoring

- [ ] Health endpoint scraped (`auradb status --addr ... --json`).
- [ ] Prometheus metrics scraped (see [OBSERVABILITY.md](OBSERVABILITY.md)).
- [ ] Disk-usage alerting on the data directory.
- [ ] Transaction-timeout and long-transaction metrics watched.
- [ ] GC metrics watched (retained versions vs. live records).
- [ ] Storage corruption warnings surfaced (run `auradb check --json` on a
      schedule and alert on `ok == false`).
- [ ] If cluster preview: per-peer replication lag and quorum availability.

### 5. Operations

- [ ] Compaction (`auradb compact`) scheduled or run under disk pressure.
- [ ] Index check/rebuild (`auradb index check` / `auradb index rebuild`).
- [ ] Statistics refresh (`auradb stats analyze`).
- [ ] GC dry-run (`auradb gc --dry-run`) understood before a real GC.
- [ ] Snapshot inspect (`auradb snapshot inspect`) for snapshot integrity.
- [ ] If cluster preview: `auradb cluster status` / `auradb cluster doctor`.

## Validation that backs this candidate

AuraDB v1.0.1 ships with, and is gated by, the following validation (see
[TESTING.md](TESTING.md)):

- storage corruption drills and a structured `auradb check --json`;
- backup/restore drills over a mixed dataset (indexes, stats, relationships,
  vectors, full-text, document-path), including post-compaction;
- upgrade drills over genuine release fixtures into v0.8.0;
- resource-limit enforcement tests;
- large-dataset smokes (CI-safe) and an on-demand 100k stress;
- a single-node soak/repeatability harness;
- performance-regression threshold tooling;
- cluster-preview recovery tests (repeated restart, snapshot install, reconnect
  storm, partition/heal, cluster doctor);
- release-artifact reproducibility checks.

None of this makes the multi-node preview production HA. Single-node mode remains
the recommended production mode.
