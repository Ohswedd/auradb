# Changelog

All notable changes to AuraDB are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project uses
[Semantic Versioning](https://semver.org/).

## [0.6.1] - 2026-06-06

Snapshot install and published-cluster smoke hardening. This patch release makes
the v0.6.0 controlled multi-node preview more reliable, observable, and
repeatable: multi-architecture Docker images, larger and concurrent-write
snapshot-install validation, snapshot-needed and follower-lag diagnostics,
cluster backup/restore dry-run planning, and a published-image cluster smoke
checklist.

This is **not** production HA. There is no production automatic failover claim,
no linearizable follower reads, no distributed transactions, no dynamic
membership, and no sharding or multi-region. Multi-node mode remains an
experimental, opt-in preview; **single-node mode remains the recommended
production mode.** All v0.6.0 behavior, the storage format, the Aura Wire
Protocol (AWP v1), and Aura Connector 0.3.x compatibility are preserved.

### Added
- Multi-architecture Docker image publishing: the tag workflow builds and pushes
  a `linux/amd64` + `linux/arm64` manifest to `ghcr.io/ohswedd/auradb:0.6.1` and
  `:latest` via Docker Buildx (arm64 built under QEMU in CI). PR/branch builds
  build `linux/amd64` through buildx without publishing. On Apple Silicon,
  `docker pull` selects the arm64 variant automatically.
- Larger snapshot-install validation: a CI-safe larger run plus `#[ignore]`d
  1,000-entry and 10,000-entry stress scenarios, asserting data, index, planner,
  and MVCC-timestamp convergence after a snapshot install.
- Snapshot install under concurrent leader writes: the leader keeps committing
  while a follower installs a snapshot and resumes AppendEntries, with no
  duplicate apply and full convergence.
- Snapshot-needed and follower-lag diagnostics: per-peer `lag_entries`,
  `needs_snapshot`, `snapshot_in_progress`, and a `catch_up_state`
  (`normal` / `probing` / `snapshot_needed` / `snapshot_installing` /
  `caught_up` / `unknown`), plus cluster-level snapshot diagnostics (last
  installed boundary, last install time, last error, bytes sent/installed,
  in-progress gauge), surfaced by `auradb cluster status --addr` and a new live
  `auradb cluster doctor --addr`.
- Additional snapshot-install metrics: `auradb_cluster_snapshot_needed_total`,
  `auradb_cluster_snapshot_bytes_sent_total`,
  `auradb_cluster_snapshot_bytes_installed_total`,
  `auradb_cluster_snapshot_in_progress`, and `auradb_cluster_snapshot_last_error`.
- Cluster backup and restore dry-run tooling: `auradb cluster backup-plan`
  inspects a data dir and reports what a logical backup would include, exclude,
  where it restores, and which secrets are referenced (redacted);
  `auradb cluster restore-plan` inspects a JSONL backup and reports what a
  restore would load. Neither writes data.
- Published GHCR cluster smoke release checklist and an enhanced
  `scripts/smoke_cluster_compose.sh` that prints the image, node ports, leader,
  quorum, peer states, and teardown result; the manual `published-image-smoke`
  workflow inspects the multi-arch manifest before running the smoke.
- Connector leader-hint UX review (docs): Aura Connector 0.3.x remains compatible
  but is not cluster-routing-aware; manual leader routing is documented, with
  tests pinning the `not_leader` leader-hint message and no-infinite-retry
  contract.

### Changed
- Improved the Docker publish workflow (multi-arch) and the cluster smoke
  documentation and release checklist.
- Improved cluster troubleshooting for followers that need a snapshot and for
  lagging followers.
- Improved observability for snapshot install and follower lag.

### Fixed
- Snapshot-install, follower-lag, Docker publish, cluster-smoke, and diagnostics
  issues found during validation are addressed; no behavioral regressions to
  single-node or single-node-cluster modes.

## [0.6.0] - 2026-06-06

Cluster ergonomics and fail-stop recovery preview. This release improves the
controlled multi-node preview experience and validates fail-stop recovery
behavior: a leader is stopped, the surviving majority elects a new leader, the
new leader accepts writes, and the old node rejoins as a follower and catches
up. It also adds the first real **peer snapshot install over the wire** (a
bounded, single-message transfer for the preview), sharper fail-stop
diagnostics, and operator runbooks for peer certificate/token rotation and
cluster backup/restore.

This is **not** production HA. There is no production automatic failover claim,
no linearizable follower reads, no distributed transactions, no dynamic
membership, and no sharding or multi-region. Multi-node mode remains an
experimental, opt-in preview; **single-node mode remains the recommended
production mode.** All v0.5.x single-node and single-node-cluster behavior, the
storage format, the Aura Wire Protocol, and Aura Connector 0.3.x compatibility
are preserved.

### Added
- Published-image Docker Compose cluster smoke workflow: `scripts/smoke_cluster_compose.sh`
  now honors `AURADB_IMAGE` so the same three-node Compose smoke can run against
  a locally built image (`auradb:0.6.0`) or a published image
  (`ghcr.io/ohswedd/auradb:0.6.0`). A manual `published-image-smoke` CI job
  verifies the published image post-release.
- Leader kill and automatic re-election preview tests: a stopped leader's term
  is taken over by the surviving majority, the new leader accepts writes, the
  old leader restarts as a follower and catches up, all nodes converge, and the
  leader-change metric increments.
- Connector write-recovery validation against a newly elected leader: after a
  leader kill, the `not_leader` response carries a leader hint and a retryable
  flag, and a client that retries against the new leader's address succeeds.
- Larger follower restart and catch-up tests across indexed, document-path,
  vector, and transaction-batch workloads, asserting no duplicate application
  and preserved MVCC timestamps and indexes after restart.
- Peer snapshot install over the wire (`InstallSnapshotRequest` /
  `InstallSnapshotResponse`): when a follower needs entries the leader has
  already compacted away, the leader sends a bounded, single-message snapshot;
  the follower validates cluster id, snapshot format, last-included index/term,
  digest, storage format, and size limit, restores atomically into a staging
  area, advances its Raft compaction boundary, and resumes AppendEntries.
  Oversized, wrong-cluster, bad-digest, and future-format snapshots are
  rejected, and a failed install preserves existing follower state.
- Cluster backup and restore runbook: leader-side logical backup
  (`auradb dump`) exports the latest committed state; `auradb restore`
  rebuilds a single-node data directory that can seed a fresh preview cluster.
- Peer certificate and token rotation runbook with rolling-restart procedures,
  CA-rotation guidance, and SAN/CA/token mismatch diagnosis.
- Improved fail-stop recovery diagnostics: cluster health now reports leader
  changes, last leader-change time and reason, per-peer last disconnect reason,
  last successful append time, snapshot install status, and `not_leader`
  rejected-write counts; surfaced via `auradb cluster diagnostics --addr` and
  `auradb cluster events --addr`.
- Replicated-cluster benchmark baseline: `benches/baseline/v0.6.0.json`.

### Changed
- Improved multi-node preview documentation, diagnostics, and operator
  workflows across the clustering, Raft, replication, operations, security,
  observability, and troubleshooting docs.
- Improved cluster readiness and leader-wait behavior and diagnostics output,
  with explicit preview and public-cluster warnings retained.

### Fixed
- Fail-stop recovery, snapshot install, connector routing, peer transport, and
  catch-up issues found during validation are addressed; no behavioral
  regressions to single-node or single-node-cluster modes.

## [0.5.2] - 2026-06-05

Multi-node preview hardening, follow-up fix. A patch release that fixes the
development certificates generated for the multi-node preview so the peer
(cluster) transport's **mutual TLS** actually works. No format or wire change;
single-node mode remains the recommended production mode.

### Fixed
- `auradb cert generate-dev` certificates were issued with a server-only Extended
  Key Usage, so a node presenting its certificate as a *client* certificate when
  dialing a peer was rejected by the peer's client-cert verifier
  ("certificate does not allow extended key usage for client authentication") and
  the multi-node TLS cluster (e.g. the Docker Compose preview) never formed a
  quorum. Generated certificates now allow **both server and client
  authentication**, which the peer transport's mutual TLS requires. Client-facing
  server TLS is unaffected. This regressed only the v0.5.1 generated-certificate
  Docker cluster path; loopback (plaintext) preview clusters were never affected.

### Changed
- The peer dialer now logs a failed connect/handshake at debug level instead of
  silently swallowing the error, so peer TLS and handshake failures are
  diagnosable.

### Added
- A regression test that forms a real two-node TLS cluster using the actual
  `auradb cert generate-dev` output, so a server-only-EKU regression is caught.

## [0.5.1] - 2026-06-05

Multi-node preview hardening. A patch release that makes the v0.5.0 controlled
multi-node preview safer, easier to operate, and more trustworthy. It adds
local Docker cluster automation, sharper cluster diagnostics, more honest
`not_leader` ergonomics, and additional leader-restart and follower-catch-up
coverage. No production-clustering claims are made: multi-node mode remains an
experimental, opt-in preview, and single-node mode remains the recommended
production mode. All v0.5.0 behavior, the storage format, the Aura Wire
Protocol, and Aura Connector 0.3.x compatibility are preserved.

### Added
- Development certificate generation for local multi-node Docker clusters:
  `auradb cert generate-dev` now accepts `--server-name` and repeatable `--san`
  flags to emit per-node certificates with explicit Subject Alternative Names,
  and `examples/cluster/generate-dev-certs.sh` drives it to produce a local CA
  and node1/node2/node3 certificates under a git-ignored `certs/` directory.
- Live Docker Compose cluster smoke validation (`scripts/smoke_cluster_compose.sh`):
  generates dev certs, starts the three-node Compose cluster, waits for a
  leader, reports status, and tears the cluster down.
- Leader restart and re-election smoke tests: a stopped leader's term is taken
  over by the surviving majority, the old leader rejoins as a follower and
  catches up, and all nodes converge.
- Larger follower catch-up tests: a follower that misses a long run of committed
  entries (including transaction batches and a compacted-log boundary) replays
  its durable log and is brought current by the leader.
- Peer TLS certificate rotation guidance and validation: documented rolling
  rotation plus tests that a wrong CA, a wrong SAN, and a peer-token mismatch are
  rejected, and that a node presenting a freshly rotated certificate is accepted.
- Better cluster failure diagnostics: `auradb cluster status --addr` now queries
  a running server for live role, leader, quorum, replication indices, and
  per-peer reachability; `auradb cluster doctor` explains no-leader, no-quorum,
  unreachable-peer, and public-cluster-without-TLS conditions.
- Replicated write latency baseline: `benches/baseline/v0.5.1.json` records the
  same-machine baseline used for regression tracking.
- Connector `not_leader` behavior validation: tests assert the leader hint, the
  retryable guidance, and that the same client connection stays usable after a
  `not_leader` response.

### Changed
- Improved multi-node preview deployment documentation across `docs/CLUSTERING.md`,
  `docs/SECURITY.md`, `docs/OPERATIONS.md`, and `docs/CLUSTER_TROUBLESHOOTING.md`.
- Improved cluster diagnostics and troubleshooting output, including explicit
  preview and public-cluster warnings.
- Improved preview guardrails and operator guidance for peer TLS and peer tokens.

### Fixed
- Peer transport, leader election, follower catch-up, Docker cluster, and
  diagnostics issues found during validation are addressed; no behavioral
  regressions to v0.5.0 single-node or single-node cluster modes.

## [0.5.0] - 2026-06-05

Controlled multi-node server preview. The first release in which AuraDB server
processes can form a real cross-process cluster, electing a leader and
replicating writes over a dedicated peer transport. This is an explicit,
experimental preview intended for local testing and early validation only;
single-node mode remains the recommended production path. Cross-process peer
networking is disabled by default and must be turned on with both
`[cluster] enabled = true` and `[cluster] experimental_multi_node = true`. All
v0.4.1 behavior, storage format, and the Aura Wire Protocol are preserved, and
Aura Connector 0.3.x remains compatible.

### Added
- Experimental cross-process peer networking: a dedicated, length-delimited,
  CRC32-checksummed peer transport with a versioned `PeerHello` handshake that
  verifies protocol version, cluster id, and node id, carries a shared peer
  authentication token, supports TLS, and returns structured `PeerError` and
  `Unsupported` responses (snapshot install is not implemented and is reported as
  unsupported rather than silently ignored).
- Static multi-node cluster preview: a fixed peer set declared in configuration
  (`[[cluster.peers]]` with `node_id` and `addr`). No join, leave, or dynamic
  membership.
- Secure peer transport baseline: loopback-only peer networking may run without
  TLS for local preview; any non-loopback peer address fails closed unless
  `allow_experimental_public_cluster = true`, which additionally requires TLS and
  a peer authentication token.
- Three-node local cluster example (`examples/cluster/`, `docker-compose.cluster.yml`)
  with per-node configs, persistent volumes, separate client and cluster ports,
  and health checks.
- Real server-process leader election over the peer transport, driven by a real
  clock in a background task.
- Real server-process replicated writes: the leader appends to its Raft log,
  replicates via AppendEntries, commits on majority, and followers apply
  committed entries.
- Follower catch-up after restart: a restarted follower replays its durable log
  and is brought current by the leader.
- Cluster status across peers: live `auradb cluster status|peers|leader` against a
  running server, including per-peer connection state, match/next index, and
  replication lag.
- Multi-node integration tests that spawn real server tasks bound to real TCP
  sockets with readiness checks and bounded polling (no fixed sleeps).
- Connector validation against the elected leader.
- Cluster troubleshooting improvements for the multi-node preview.
- New cluster CLI commands: `auradb cluster leader`, `auradb cluster wait-ready`,
  and `auradb cluster wait-leader` (with `--timeout-secs`, `--json`, and auth/TLS
  flags).
- New peer and Raft metrics (`auradb_peer_connected`,
  `auradb_peer_replication_lag_entries`, `auradb_raft_elections_total`,
  `auradb_raft_election_timeouts_total`, `auradb_raft_append_entries_failures_total`,
  `auradb_raft_heartbeat_latency_ms`, `auradb_cluster_quorum_available`).

### Changed
- Cluster mode can now run with static peers when explicitly enabled via
  `experimental_multi_node = true`; without that flag, a non-empty peer set still
  fails closed exactly as in v0.4.1.
- Cluster diagnostics include peer reachability and replication state, and the
  cluster doctor warns about preview mode, public-cluster mode, missing leader or
  quorum, lagging or unreachable peers, and unsupported snapshot install.

### Fixed
- Cluster networking, Raft, replication, catch-up, and status bugs found during
  multi-node validation.

## [0.4.1] - 2026-06-05

Raft durability and cluster-mode hardening. A patch release that strengthens the
Raft and replication groundwork from v0.4.0 before any real cross-process
multi-node preview. No storage-format or wire-protocol change: multi-node server
deployment remains experimental and disabled by default, and single-node mode
remains the recommended production path. All v0.4.0 behavior is preserved.

### Added
- Raft log compaction boundary validation: a compactable-prefix calculation that
  refuses to discard entries that are not safely applied or are beyond the
  committed index, preserves the last included index and term, persists
  `raft-compaction.json`, and surfaces a structured `Compacted` error for reads
  before the retained prefix. AppendEntries consistency checks understand the
  compacted prefix.
- Snapshot restore edge-case tests and a richer snapshot manifest (cluster id,
  node id, storage-format version, created-at timestamp), with restore that is
  atomic (build in a temporary directory, validate, then swap), refuses to
  overwrite a non-empty target without `--force`, and rejects future formats,
  cluster-id mismatch, corrupt manifests, and digest mismatches.
- Raft apply idempotency tests under restart and crash-like sequences (commit
  before apply, partial apply, apply before watermark update).
- Cluster metadata corruption tests (missing, malformed, future-format, and
  partial identity) that fail closed.
- Stronger peer configuration validation: duplicate peers, a peer equal to the
  local node id, and any non-empty peers list are rejected with clear errors in
  this release (cross-process peers are not enabled).
- Single-node cluster overhead benchmarks (`benches/baseline/v0.4.1.json`,
  `auradb-cluster` `cluster_overhead` bench) comparing direct and single-node
  cluster write/read paths for same-machine regression tracking.
- Deterministic multi-node partition tests (minority cannot commit, majority
  elects a leader, old leader steps down on rejoin, committed entries survive a
  leader change, an uncommitted old-leader entry never commits and is repaired
  away).
- Cluster troubleshooting documentation
  ([docs/CLUSTER_TROUBLESHOOTING.md](docs/CLUSTER_TROUBLESHOOTING.md)).
- Cluster operational diagnostics: `auradb cluster compact-log [--dry-run]
  [--json]` and `auradb snapshot create|inspect|restore`.

### Changed
- Improved `auradb cluster status` / `auradb cluster doctor` output (JSON modes,
  clearer peer-rejection and durability warnings).
- Improved Raft durability checks around the compaction boundary and metadata.
- Improved cluster-mode documentation and release guardrails.

### Fixed
- Hardened fail-closed handling of corrupt cluster metadata, corrupt Raft
  compaction metadata, and inconsistent snapshot manifests found during
  validation.
- Gave each benchmark run a unique scratch directory (process id plus a per-call
  counter) so concurrent `auradb bench` runs in one process no longer race on a
  shared temporary path.

## [0.4.0] - 2026-06-05

The replication and Raft foundation for future clustered deployments. This
release introduces a correct, durable, testable cluster foundation. **Single-node
mode remains the recommended production path.** Multi-node clustering is
experimental: the Raft and replication core is validated by deterministic
in-process tests, but cross-process multi-node server deployment is not enabled
(configuring peers is rejected at startup). When cluster mode is disabled — the
default — all v0.3.1 behavior is preserved byte-for-byte.

### Added
- Stable node identity (`NodeId`) and cluster identity (`ClusterId`), persisted
  under `<data_dir>/cluster/` and created by `auradb init`.
- Cluster metadata and configuration (the `[cluster]` config table), validated
  at startup; unknown future metadata formats are rejected (fail closed).
- A durable, checksummed Raft log abstraction with corruption detection and
  crash-safe recovery (`auradb-raft`).
- A minimal, deterministic Raft state machine: follower/candidate/leader roles,
  elections, `RequestVote`, `AppendEntries`, heartbeats, log repair, and commit
  advancement, driven by a logical test clock.
- Single-node Raft mode: when cluster mode is enabled with no peers, every write
  is ordered through a durable local Raft log and replayed on restart.
- A leader-and-follower role model with a leader-only write path; followers
  reject writes with a structured `not_leader` error and a leader hint.
- A replicated command model and an idempotent replicated apply path; the MVCC
  commit timestamp is the Raft log index, so replicas derive identical ordering.
- A versioned snapshot boundary (`SnapshotManifest`) for future state transfer,
  with local create and restore.
- Cluster status and diagnostics: `auradb cluster init|status|peers|doctor|
  bootstrap`, plus cluster fields in `auradb status --json`, `auradb doctor`,
  and the server health report.
- Replication and Raft metrics (term, commit/applied/last-log index, leader
  changes, votes, AppendEntries counters, replication lag, apply errors, apply
  latency).
- Deterministic Raft and replication tests, including in-process multi-node
  consensus and replicated-apply tests, plus a single-node cluster example
  config (`examples/auradb.cluster.local.toml`).

### Changed
- The internal write path can be routed through the replication layer when
  cluster mode is enabled; the default (cluster-disabled) path is unchanged.
- Server health and status include an additive `cluster` section. The Aura Wire
  Protocol version is unchanged; Aura Connector 0.3.x remains fully compatible.

### Fixed
- Replication, recovery, and cluster-mode correctness issues found during
  validation (idempotent apply on restart; commit-order preservation through the
  Raft log; fail-closed handling of unknown future cluster, Raft, and snapshot
  formats).

## [0.3.1] - 2026-06-05

MVCC stabilization, upgrade confidence, and operational guardrails. A
stabilization release before replication and Raft work: it hardens the MVCC
transaction lifecycle so a long-lived or abandoned transaction can no longer pin
versions forever without visibility, adds transaction timeouts and an
abandoned-transaction reaper, strengthens GC validation, and surfaces MVCC
pressure through metrics, status, and `doctor` warnings. All v0.3.0 behavior is
preserved and Aura Connector 0.3.x remains compatible (no connector release is
required). This release still implements snapshot isolation, **not** serializable
isolation, and adds no clustering, replication, or Raft.

### Added

- Transaction timeout and abandoned transaction cleanup: an idle transaction past
  `[mvcc] transaction_timeout_secs` is reaped by the abandoned-transaction reaper,
  its snapshot released and further operations rejected with a structured
  `transaction_timeout` error.
- Active transaction registry tracking id, read timestamp, start time, last
  activity, connection id, and state; GC reclaims from this registry, never stale
  leaked state.
- MVCC pressure metrics: `auradb_mvcc_active_transactions`,
  `auradb_mvcc_oldest_snapshot_age_seconds`, `auradb_mvcc_retained_versions`,
  `auradb_mvcc_gc_runs_total`, `auradb_mvcc_gc_reclaimed_versions_total`,
  `auradb_mvcc_gc_reclaimed_bytes_total`, `auradb_mvcc_transaction_timeouts_total`,
  and `auradb_mvcc_conflicts_total`.
- Operational warnings in `auradb doctor` for long-lived snapshots, version
  pressure, disabled GC, disabled transaction timeouts, and stale statistics.
- Stronger MVCC garbage collection validation, plus `auradb gc --dry-run` and
  `auradb gc --json`, and `bytes_reclaimed` in the GC report.
- Additional upgrade safety tests across genuine v0.1.0, v0.2.0, v0.2.1, and
  v0.3.0 release fixtures into v0.3.1.
- Query planner regression tests and backup/restore-with-GC tests.
- Benchmark regression baseline comparison: `auradb bench compare --baseline … --current …`
  with an optional `--fail-threshold-percent` for CI.
- Improved `EXPLAIN ANALYZE` output: estimated-vs-actual rows, planner-stats
  version, selected-index reason, MVCC snapshot timestamp, and a stale-statistics
  warning (all additive JSON fields).

### Changed

- Improved cleanup behavior for dropped or disconnected transactions: a
  connection's transactions are rolled back on disconnect, and the reaper releases
  any that are abandoned.
- Health and `status` now report active snapshots and MVCC pressure (additive
  `mvcc` section in the health report).
- Improved documentation for snapshot isolation and version retention.

### Fixed

- An abandoned transaction handle (dropped without commit or rollback) no longer
  pins MVCC versions indefinitely: the abandoned-transaction reaper releases it.

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
