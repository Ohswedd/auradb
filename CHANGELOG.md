# Changelog

All notable changes to AuraDB are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project uses
[Semantic Versioning](https://semver.org/).

## [0.3.0] - 2026-06-05

MVCC and query planner foundations. AuraDB now stores multiple committed
versions of each record and serves transactional reads from a snapshot pinned at
`begin`, giving **single-node snapshot isolation** with optimistic write-conflict
detection. Query reads route through a cost-based planner that uses persisted
statistics to choose an access path, and `EXPLAIN ANALYZE` reports measured
execution metrics. The on-disk storage format moves to v2; a v0.1.0/v0.2.x
directory is migrated transparently on first open. This release preserves all
v0.2.1 behavior for non-transactional reads and remains compatible with Aura
Connector 0.3.x (no connector release is required).

This release implements snapshot isolation, **not** serializable isolation.

### Added

- MVCC record versions: each record id maps to an ordered version chain, and a
  delete is a committed tombstone version. Versions, timestamps, and tombstones
  survive restart.
- Snapshot isolation with transaction read timestamps pinned at `begin`: a
  transaction sees committed state as of its begin-time snapshot (plus its own
  staged writes) and does not observe writes committed by other transactions
  after it began.
- Optimistic write-conflict detection (first-committer-wins): commit aborts with
  `Error::Conflict` when a record the transaction wrote was modified by a
  transaction that committed after the snapshot was pinned (covers write-write,
  update-delete, and delete-update conflicts).
- Version garbage collection (`auradb gc`, plus optional background GC): reclaims
  versions no active transaction can observe and drops fully-deleted records,
  always retaining the latest version and at least `min_retained_versions`.
- Query planner with costed index selection: a plan tree (point lookup, index
  lookup, document-path / full-text index lookup, vector search, scan, and the
  filter/sort/limit/offset/projection/relationship-include operators) chosen by
  estimated cost from row counts and per-field cardinality.
- Persisted planner statistics (`planner_stats.json`): row counts, field
  cardinality, vector counts, full-text document counts, and average record size,
  recomputed by `auradb stats analyze` and kept current on each mutation.
- `EXPLAIN ANALYZE` with execution metrics: scanned/matched/returned rows,
  execution and planning time, the index used, and the snapshot timestamp when
  run inside a transaction. Carried over the wire as an optional flag in the raw
  Query IR, so no protocol break.
- New CLI commands: `auradb gc`, `auradb stats analyze`, `auradb stats show`.
- New `[mvcc]` server configuration: `gc_enabled`, `gc_interval_secs`,
  `min_retained_versions`.
- MVCC, planner, and `EXPLAIN ANALYZE` benchmarks (`benches/mvcc.rs`,
  `benches/planner.rs`, `benches/explain_analyze.rs`) and a v0.3.0 baseline.
- Transaction isolation, planner, and `EXPLAIN ANALYZE` conformance scenarios.
- Upgrade tests from real v0.2.0 and v0.2.1 release fixtures to v0.3.0.

### Changed

- Transaction reads now use a stable begin-time snapshot instead of reading the
  latest committed state.
- Query execution now routes through the planner before execution.
- Index selection now considers statistics and estimated cost, choosing the most
  selective index among candidates and a full scan when no index applies.
- The on-disk storage format is now v2 (commit-timestamped version chains). A
  v1 (≤ 0.2.x) directory is migrated to v2 on first open; an unknown future
  format is still rejected.

### Fixed

- Transactional reads no longer observe writes committed by other transactions
  after the reading transaction began (previously they saw the latest committed
  state).

## [0.2.1] - 2026-06-05

Operational polish, safer defaults, release confidence, and deployment
readiness. This patch release preserves all v0.2.0 behavior; it adds deployment
examples, an operational token-rotation command, and durability and
compatibility coverage in CI.

### Added

- Secure Docker Compose example (`docker-compose.secure.yml`) that runs AuraDB
  with authentication and TLS enabled, a non-root user, a mounted config, a data
  volume, a mounted certificate directory, and a healthcheck. The token hash is
  supplied through an environment variable rather than committed in plaintext.
- Production configuration templates: `examples/auradb.secure.toml` (auth and
  TLS enabled, redacted token-hash placeholder) and `examples/auradb.local.toml`
  (loopback, auth and TLS disabled, development only), plus an
  `examples/production/` deployment bundle.
- Token rotation support: `auradb auth rotate-token` re-hashes a new token with
  Argon2id, writes the configuration atomically, preserves unrelated fields,
  optionally backs up the previous configuration, validates the result, and
  never writes a plaintext token.
- Backup and restore verification: an integration test that dumps a database
  containing scalar, document, vector, relationship, full-text, and
  document-path data and restores it into a fresh data directory, then verifies
  records, schema, indexes, and search.
- Upgrade coverage from an AuraDB v0.1.0 data directory: a committed fixture
  written by the v0.1.0 binary is opened by v0.2.1, validated, and its indexes
  rebuilt, with `auradb check` passing afterward.
- Chaos restart test that drives writes, updates, and deletes against the engine
  with deterministic crash-and-reopen cycles and compares the recovered state
  against a reference model.
- Connector compatibility smoke script
  (`tests/conformance/python/run_connector_smoke.py`) that runs a minimal real
  Aura Connector scenario against a running server.
- Benchmark baseline snapshot (`benches/baseline/v0.2.1.json`) produced by
  `auradb bench --json`, with `docs/BENCHMARKS.md`.
- JSON output for `auradb status`, `auradb doctor`, and `auradb bench`
  (`--json`), and a richer health and readiness report.

### Changed

- Improved Docker security defaults and deployment documentation; the secure
  Compose example is now the recommended deployment path.
- `auradb dump` accepts `--output` (alias of `--out`) and `auradb restore`
  accepts `--input` (alias of `--in`) for consistency with the documentation.
- Improved release-validation and operational health-check documentation.

### Fixed

- Pinned the Docker build stage to `rust:1.90-slim-bookworm` so its glibc matches
  the `debian:bookworm-slim` runtime. The unpinned `rust:1.90-slim` tag had moved
  to a newer Debian, producing an image whose binary failed at startup with a
  missing-glibc-version error.
- `auradb dump` now writes collections in dependency order so that a
  relationship target is restored before the collection that references it;
  restoring a dump with relationships no longer depends on collection ordering.
- Documentation consistency and version references across the README and the
  `docs/` tree.

## [0.2.0] - 2026-06-04

Single-node release focused on security, durability hardening, and public
usability.

### Added

- **Authentication.** Enforced static-token authentication. An `[auth]` config
  block (`enabled`, `mode = "static-token"`, `token_hash`,
  `token_hash_algorithm = "argon2id"`) gates every schema, query, mutation,
  cursor, explain, migration-estimate, and transaction operation when enabled.
  Tokens are verified against an Argon2id PHC hash with constant-time
  comparison and are never stored in plaintext. Clients may authenticate via an
  `auth_token` in the HELLO handshake or a dedicated AUTH frame (opcode `0x04`,
  returning `AuthResult` `0x84`). Only HELLO, AUTH, PING, and HEALTH are allowed
  unauthenticated. Generate a hash with `auradb auth hash-token`.
- **TLS.** Server-terminated TLS (rustls) via a `[tls]` config block (`enabled`,
  `cert_path`, `key_path`, `client_ca_path`, `require_client_cert`), including
  mutual TLS. Generate development-only certificates with
  `auradb cert generate-dev`. Clients trust the CA with `--tls-ca`.
- **Persisted indexes.** Indexes are snapshotted to an `indexes/` directory
  (`INDEX_MANIFEST.json` plus framed, CRC32-checked per-collection `.idx` files)
  at checkpoints (`auradb compact`, graceful shutdown, `auradb index rebuild`).
  On open, a snapshot loads only when its content fingerprint, schema field
  shape, and CRC all match; otherwise the engine safely rebuilds from storage.
  Persisted kinds: primary key, unique, secondary, document-path, full-text, and
  exact vector. New `auradb index check` and `auradb index rebuild` commands.
- **Document-path indexes.** Declared in a schema via
  `{ "path": "profile.company", "kind": "document_path" }`. Accelerates equality
  filters on nested document values addressed by a dotted path; reported in
  EXPLAIN as `strategy: index_lookup` with `used_index`.
- **Full-text search.** Declared via `{ "path": "body", "kind": "full_text" }`.
  Case-folded tokenizer split on non-alphanumeric boundaries with no stop-word
  removal. A `contains_text` filter matches records that contain every distinct
  query token (boolean AND), ranked by summed term frequency (not BM25). EXPLAIN
  reports `strategy: full_text_scan`; without an index it falls back to a
  tokenized `full_scan`.
- **Transaction-scoped reads.** Reads issued with a transaction id now execute
  against the transaction view — committed state overlaid with the transaction's
  own staged writes and deletes — across `find`, `filter`, `count`, `exists`,
  `explain`, vector nearest, document-path filters, full-text search,
  relationship `include`, and cursor paging. A transaction sees its staged
  inserts and updates and does not see its staged deletes (read-your-writes);
  the effects stay invisible to non-transactional readers until commit. Index
  seeding (equality, vector, full-text) is served from an overlay index built
  over the transaction view, so a staged write is never missed and a staged
  delete is never returned. This removes the prior limitation that reads inside a
  transaction ignored the transaction id and reflected only committed state.
  Covered by `crates/auradb/tests/transactions.rs`, the
  `transactional_read_sees_staged_write_over_the_wire` server test, and the
  `transaction_scoped_reads` conformance scenario.
- **Security defaults.** A non-loopback bind with auth disabled is rejected at
  startup unless `allow_insecure_bind = true` (config) or `--allow-insecure-bind`
  is passed. `auradb doctor` prints a redacted security summary.
- **CLI.** `auth hash-token`, `cert generate-dev`, `config validate`,
  `compatibility`, `index check`, `index rebuild`; `status` gains `--token`,
  `--tls-ca`, `--tls-server-name`; `server` gains `--allow-insecure-bind`.
- **Server capabilities.** New advertised capabilities: `authentication`, `tls`,
  `persisted_indexes`, `document_path_indexes`, `full_text_search`.
- **Recovery testing.** Deterministic, seeded recovery and corruption tests
  covering randomized insert/update/delete against a reference model (with and
  without checkpoint), trailing-segment truncation, mid-batch byte-flip
  detection, catalog corruption detection, and corrupt/missing index file and
  manifest repair (`crates/auradb-storage/tests/recovery.rs`,
  `crates/auradb/tests/recovery.rs`).
- **Distribution.** A published Docker image at `ghcr.io/ohswedd/auradb`
  (non-root, healthcheck, `/data` volume) and prebuilt binary release artifacts
  for Linux, macOS, and Windows targets with a `SHA256SUMS` file, produced by the
  `release.yml` workflow on `v*` tags.

### Changed

- AWP gains additive fields and opcodes (optional HELLO `auth_token`;
  `auth_required` and `authenticated` in HELLO_ACK; AUTH/AUTH_RESULT opcodes;
  `unauthenticated` and `invalid_credentials` error codes). The 44-byte framed
  header, magic, version, and checksums are unchanged and backward compatible.
- The Python conformance harness gains `--auth-token`, `--tls-ca`, and
  `--tls-server-name`, and new document-path, full-text, and EXPLAIN scenarios.
- New CI workflows: `conformance.yml` (auth disabled, auth enabled with a
  rejection check, and TLS) and `docker.yml` (build, smoke, and GHCR publish).

### Security

- Tokens are stored only as Argon2id hashes and verified in constant time;
  secrets are never logged or echoed in error frames.
- `auth.enabled = true` without `token_hash`, a malformed `token_hash`, missing
  or invalid TLS material, or `require_client_cert = true` without
  `client_ca_path` all fail startup (fail closed).
- Failed authentication increments the `auradb_auth_failures_total` metric.

## [0.1.0] - 2026-06-04

First single-node developer release.

### Added

- **Storage engine.** Append-only, checksummed segment log with a manifest,
  crash recovery (torn-tail truncation, corruption detection), and compaction.
- **Aura Wire Protocol.** Binary framed protocol with version negotiation,
  header and payload CRC32 checksums, request-id correlation, and structured
  error frames.
- **Transactions.** Buffered write sets with optimistic write and read conflict
  detection, atomic durable commit, and rollback.
- **Schema catalog.** Typed fields, primary keys, unique and secondary indexes,
  document and vector fields, relationships, and validation.
- **Query engine.** Find, filter (comparisons, `contains`, `AND`/`OR`/`NOT`),
  order/limit/offset, projection, count, exists, insert, bulk insert, update,
  delete, upsert, relationship includes, document path access, exact vector
  nearest-neighbour search, and EXPLAIN.
- **Migration impact estimation.**
- **Server-side cursors** with paging and idle-timeout reaping.
- **Server.** Async TCP listener, concurrent connections, payload limits,
  graceful shutdown, and per-connection transactions.
- **Observability.** Metrics registry (counters, gauges, latency histograms)
  with JSON and Prometheus-text export, plus structured tracing.
- **CLI.** `version`, `init`, `server`, `doctor`, `status`, `check`, `compact`,
  `dump`, `restore`, `bench`.
- **Conformance harness.** A protocol client and scenario suite, plus a Python
  harness.
- Docker support, example configuration, benchmarks, and GitHub Actions CI.

### Not yet implemented (not claimed)

Distributed clustering, replication, sharding, failover, multi-region, and Raft;
approximate (ANN/HNSW) vector indexes; BM25 full-text and hybrid fusion ranking;
serializable MVCC; enforced TLS and authentication; field-level encryption,
RBAC; time travel; and change streams. See [docs/ROADMAP.md](docs/ROADMAP.md).

[0.2.1]: https://github.com/Ohswedd/auradb/releases/tag/v0.2.1
[0.2.0]: https://github.com/Ohswedd/auradb/releases/tag/v0.2.0
[0.1.0]: https://github.com/Ohswedd/auradb/releases/tag/v0.1.0
