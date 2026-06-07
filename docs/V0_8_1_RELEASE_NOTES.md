# AuraDB v0.8.1 release notes

**Production-readiness stabilization patch for the v0.8.0 candidate.**

AuraDB v0.8.1 is a narrow stabilization patch. It hardens the operational edges
of the v0.8.0 production-readiness candidate — backup/restore corner cases,
resource-limit boundaries, soak ergonomics, release-artifact verification, and
runbook clarity — and adds **no** product features. It changes **no** Raft,
storage, query, MVCC, replication, or snapshot semantics except where a
documented bug is fixed.

It is **not** production HA. **Single-node mode remains the recommended
production mode.** Multi-node remains an experimental, opt-in preview.

## Compatibility

- **Storage format unchanged** (manifest `format_version` 2). Data directories
  from every prior release open in place; v0.8.0 directories need no migration.
- **Aura Wire Protocol unchanged** (AWP 1).
- **Aura Connector v0.4.1 compatible** (and v0.3.x / v0.4.0 as before). This is an
  AuraDB-only patch; the connector is unchanged.
- All v0.8.0 behavior is preserved except where a documented bug is fixed.

## Highlights

### More backup and restore edge-case coverage

New `backup_restore_edge_cases` tests round-trip the awkward corners through
`dump → verify → restore`: an empty database, a schema-only export, a large
single record, Unicode and escaped strings (including emoji, quotes, backslashes,
and a NUL), deeply nested documents, vectors, relationship delete policies,
full-text fields with punctuation and mixed case, and document-path indexes after
restore. The rejection contract is pinned too: malformed JSONL, records for an
undeclared collection, truncated files, invalid schema sections, and the per-line
restore size bound are all refused with structured errors, and `backup verify`
never echoes record contents.

### `backup verify` rejects duplicate primary keys

A faithful `auradb dump` exports exactly one record per primary key (the latest
visible MVCC state). `auradb backup verify` now rejects a backup that carries two
records with the same primary key — a corrupt or hand-edited dump whose restore
would silently collapse two logical records into one (data loss). The report
names only the collection and a count; the duplicated key value is never printed,
preserving the existing redaction guarantee. Restore semantics are unchanged
(restore remains an idempotent upsert); the duplicate guard lives in `verify`,
which is the gate operators run before trusting a backup.

### More resource-limit edge-case coverage

New limit tests cover exact-boundary acceptance, one-past-boundary rejection,
zero and absurd configuration values (zero is rejected at validation; resource
bounds carry no upper cap and are operator policy), error-shape stability,
payload redaction (a secret embedded in a request is never echoed in a limit
error), the no-partial-commit guarantee when a staged write is refused for
exceeding the transaction write-set bound, and a structured error for a
single-message snapshot payload that exceeds the peer-transfer size limit.

The document-depth limit error now **names the offending top-level field** so an
operator can find the over-nested path without bisecting the record. The field
name is structural, not record content, so this leaks nothing.

### Better soak scripts and logs

`scripts/soak_single_node.sh` and `scripts/soak_cluster_preview.sh` now stamp
every line with a timestamp, print the binary version and the data/log
directories up front, honor `KEEP_ARTIFACTS=1` to retain artifacts on success
(and always preserve everything on failure), allow a `LEADER_ADDR` override for
the cluster preview, and emit a final machine-readable summary line
(`result=PASS cycles=… elapsed_secs=…`). Both still exit non-zero on the first
mismatch.

### Stronger release-artifact verification

`scripts/verify_release_artifacts.sh` gained a SHA256SUMS-completeness check (no
stray, unlisted asset can ship in a release), a `--tag` release-body
honesty-wording check (a scoped release must keep the single-node-recommended /
preview-is-not-production-HA framing), and a network-free `--self-test` mode that
synthesizes artifact directories and asserts that a good set passes while a
missing archive, a bad checksum, and a wrong-version name each fail.

## Upgrading

v0.8.1 is a drop-in patch over v0.8.0. There is no storage migration. As always,
take a backup and run a restore drill before upgrading a production deployment;
see [`docs/UPGRADING.md`](UPGRADING.md) and [`docs/RUNBOOKS.md`](RUNBOOKS.md).

## What this release is not

v0.8.1 does **not** claim production clustering, production automatic failover,
linearizable follower reads, distributed transactions, dynamic membership,
sharding, multi-region, serializable isolation, ANN/HNSW, BM25, or hybrid fusion.
None of those are present. Single-node mode remains the recommended production
candidate path; multi-node mode remains an experimental preview.
