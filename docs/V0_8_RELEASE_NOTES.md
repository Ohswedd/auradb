# AuraDB v0.8.0 release notes

**Production-readiness candidate for single-node deployments and a stronger
cluster preview.**

AuraDB v0.8.0 moves the project from "impressive preview" toward "credible early
production candidate" for **single-node** mode — without overclaiming. It is a
hardening, validation, and operability release: it introduces **no** large new
database features and changes **no** Raft, storage, query, MVCC, replication, or
snapshot semantics except where a real bug was fixed during hardening.

It is **not** production HA. **Single-node mode remains the recommended production
mode.** Multi-node remains an experimental, opt-in preview.

## Compatibility

- **Storage format unchanged** (manifest `format_version` 2). Data directories
  from every prior release open in place; upgrade drills validate v0.1.0 through
  v0.7.1 fixtures into v0.8.0.
- **Aura Wire Protocol unchanged** (AWP v1).
- **Aura Connector v0.4.1 compatible** (and v0.3.x / v0.4.0 as before). This is an
  AuraDB-focused release; the connector is unchanged.
- All v0.7.1 behavior is preserved.

## Highlights

### Single-node production-readiness checklist

New [`docs/PRODUCTION_READINESS.md`](PRODUCTION_READINESS.md) states support
levels honestly (recommended single-node candidate; experimental cluster preview;
explicitly unsupported HA/sharding/multi-region/ANN/BM25) and provides secure
configuration, backup, upgrade, monitoring, and operations checklists.

### Storage corruption drills and a structured `check`

`auradb check --json` now emits a structured report covering storage (manifest,
segments, format version), catalog, indexes, planner statistics, and any cluster
Raft/snapshot state, with `warnings`/`errors` and an overall `ok` flag (non-zero
exit on failure). Corruption drills cover segment checksum mismatch, manifest /
catalog / index-manifest / planner-stats / raft-log / snapshot-boundary
corruption, and rejection of unknown future formats. The report never prints
secrets.

### Backup and restore drills, and `backup verify`

New `auradb backup verify --input <file> --json` validates a JSONL dump without
importing it (every line parses, the per-line size bound holds, records reference
declared schemas). Backup/restore drills round-trip a mixed dataset (scalar,
document, full-text, document-path, vector, relationship, indexed) through dump →
verify → restore → `check`, including after compaction, and reject corrupt input.

### Upgrade drills across genuine release fixtures

The v0.8.0 upgrade checklist (open → check → analyze → index check → backup →
verify → restore → query smoke) runs over genuine release-binary fixtures
covering both on-disk storage families, with unknown future formats rejected.

### Resource limits and defensive bounds

A new `[limits]` configuration section adds real, enforced bounds — query
limit/offset, full-text query tokens, document nesting depth, vector
dimensionality, and transaction write-set size — plus a backup-input line bound
in `restore`. Violations return a structured `limit_exceeded` error and never
tear down the connection. The wire-level frame bound (`max_payload_bytes`) is
unchanged.

### Large-dataset, soak, and performance tooling

CI-safe large-dataset smokes (10,000 records across all field/index kinds) plus
an on-demand 100k stress; a single-node soak harness (`scripts/soak_single_node.sh`)
and a cluster-preview soak harness (`scripts/soak_cluster_preview.sh`); and
performance-regression threshold tooling (`auradb bench compare
--fail-threshold-percent` and `scripts/compare_benchmarks.py`) with a v0.8.0
baseline.

### Security hardening review

[`docs/SECURITY.md`](SECURITY.md) gains a hardening checklist (auth/TLS, token and
cert rotation, no secrets in logs or diagnostics, non-root container, dependency
audit policy). Redaction is covered by tests across `doctor`, `status`,
`config validate`, and `check`.

### Cluster preview recovery hardening and runbooks

The multi-node preview's recovery coverage (repeated leader restart, snapshot
install after compaction, reconnect storm, partition/heal, cluster doctor) is
documented and exercised. New [`docs/RUNBOOKS.md`](RUNBOOKS.md) gives 20 operator
runbooks. None of this makes the preview production HA.

### Release artifact reproducibility

`scripts/verify_release_artifacts.sh` verifies a local or published release: all
target archives present, `SHA256SUMS` complete and matching, versioned names, and
the host binary reports the expected version.

## Not in this release (and not claimed)

Production automatic failover, production clustering, linearizable follower reads,
distributed transactions, dynamic membership, sharding, multi-region, serializable
isolation, ANN/HNSW, BM25, hybrid fusion, a Kubernetes operator, or new language
SDKs. See [ROADMAP.md](ROADMAP.md).

## Upgrading

Read [UPGRADING.md](UPGRADING.md). In short: back up first
(`auradb dump` + `auradb backup verify`), stop the server, swap the binary, run
`auradb check --json`, and start. Roll back by restoring the previous binary and,
if needed, the pre-upgrade backup.
