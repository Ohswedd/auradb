# AuraDB v0.4.1 release notes

**Theme: Raft durability and cluster-mode hardening.**

AuraDB v0.4.1 is a patch release that hardens the Raft and replication groundwork
introduced in v0.4.0, before any real cross-process multi-node preview. It changes
no on-disk format and no wire protocol. **Multi-node server deployment remains
experimental and disabled by default; single-node mode remains the recommended
production path.** When cluster mode is disabled — the default — all v0.4.0
behavior is preserved.

## At a glance

- **Raft log compaction boundaries.** The durable Raft log can now compute a
  compactable prefix and discard entries a snapshot covers, recording the last
  included index and term in `raft-compaction.json`. Compaction refuses to discard
  entries that are not yet applied or that lie beyond the committed index, reads
  before the retained prefix return a structured `Compacted` error, and the
  AppendEntries consistency check understands the compacted boundary. Compaction
  metadata is persisted across restarts and fails closed if corrupt.
- **Snapshot restore edge cases.** The snapshot manifest now records the cluster
  id, node id, storage-format version, captured collection/record counts, and a
  creation timestamp. Restore is **atomic** — it builds into a staging directory,
  validates, and swaps into place — and refuses to overwrite a non-empty target
  without `--force`. It rejects future formats, cluster-id mismatches, corrupt
  manifests, and digest mismatches before touching existing data.
- **Apply idempotency under restart.** Committed entries apply exactly once across
  restarts; crash-like sequences (commit-before-apply, partial apply, apply before
  the watermark update) recover without duplicating records, and uncommitted
  entries are never applied.
- **Cluster metadata corruption handling.** Missing, malformed, future-format, and
  partial cluster identity is rejected (fail closed) rather than silently
  re-initialized.
- **Stronger peer configuration validation.** Duplicate peers and a peer pointing
  at the node's own address are rejected with clear errors. Any non-empty peers
  list is still rejected at startup in this release: cross-process multi-node
  deployment is not enabled.
- **Deterministic multi-node partition tests.** Minority partitions cannot commit;
  the majority elects a leader; a rejoining old leader steps down; committed
  entries survive a leader change; an uncommitted old-leader entry is repaired
  away. All driven by the in-process simulation with a logical clock — no flaky
  sleeps.
- **`not_leader` behavior validated over the wire.** A write on a non-leader node
  returns a structured `not_leader` error with a leader hint, the connection stays
  healthy, and the response is prompt and terminal (no internal retry loop). Aura
  Connector 0.3.x maps the code safely and needs no change.
- **Operational diagnostics.** New `auradb cluster compact-log [--dry-run]
  [--json]` and `auradb snapshot create|inspect|restore` commands.
- **Single-node cluster overhead benchmarks** for same-machine regression
  tracking (`benches/baseline/v0.4.1.json`, the `cluster_overhead` bench).
- **Cluster troubleshooting documentation**
  ([CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md)).

## New CLI commands

```bash
# Inspect and compact the durable Raft log (single-node cluster).
auradb cluster compact-log --data-dir <dir> --dry-run --json
auradb cluster compact-log --data-dir <dir> --json

# Capture, inspect, and restore a portable snapshot.
auradb snapshot create  --data-dir <dir> --output <file>
auradb snapshot inspect --input <file>
auradb snapshot restore --input <file> --data-dir <dir> [--force]
```

## Compatibility

- **Storage format:** v2 — unchanged.
- **Cluster metadata format:** v1 — unchanged.
- **Snapshot manifest format:** v1 — the new fields are additive and optional, so
  a v0.4.0 manifest decodes unchanged.
- **Aura Wire Protocol:** AWP 1 — unchanged. The `cluster` health section and the
  `not_leader` error code are additive, exactly as in v0.4.0.
- **Aura Connector:** 0.3.x remains fully compatible. No connector release is
  required. The connector conformance was executed against the **published**
  `aura-connector` 0.3.0 (from PyPI) in both non-cluster and single-node cluster
  modes — the connector smoke passing 12/12 and the full Python wire conformance
  18/18 in each mode, with the additive `cluster` health section and `not_leader`
  error code handled safely. See [CONFORMANCE.md](CONFORMANCE.md).
- **Upgrades:** v0.1.0, v0.2.0, v0.2.1, v0.3.0, v0.3.1, and v0.4.0 data directories
  open unchanged. See [UPGRADING.md](UPGRADING.md).

## Not in this release (and not claimed)

Production multi-node clustering, automatic failover, linearizable follower reads,
distributed transactions, sharding, multi-region, and serializable isolation are
**not** implemented and **not** claimed. A single-node cluster provides no fault
tolerance. See [CLUSTERING.md](CLUSTERING.md) and [ROADMAP.md](ROADMAP.md).
