# Observability

`auradb-observability` provides structured tracing and a metrics registry. No
external collector is required to run the server.

## Tracing

`init_tracing(level, json)` installs a `tracing-subscriber` with an env-filter
directive (e.g. `info`, `auradb=debug`). With `log_json = true`, logs are emitted
as structured JSON with request ids, error codes, and span context. The call is
idempotent.

## Metrics

The `Metrics` registry tracks:

- **Counters** - `requests_total`, `errors_total`, `queries_total`,
  `mutations_total`, `bytes_read`, `bytes_written`.
- **MVCC counters** - `auradb_mvcc_gc_runs_total`,
  `auradb_mvcc_gc_reclaimed_versions_total`, `auradb_mvcc_gc_reclaimed_bytes_total`,
  `auradb_mvcc_transaction_timeouts_total`, `auradb_mvcc_conflicts_total`.
- **Gauges** - `active_connections`, `active_transactions`, `active_cursors`.
- **MVCC gauges** - `auradb_mvcc_active_transactions`,
  `auradb_mvcc_oldest_snapshot_age_seconds`, `auradb_mvcc_retained_versions`.
- **Histograms** - `request_latency`, `query_latency`, `storage_latency`
  (fixed microsecond buckets with sum and count).
- **Cluster / Raft metrics (v0.4.0, present when cluster mode is enabled)** -
  `auradb_cluster_enabled`, `auradb_node_role`, `auradb_raft_current_term`,
  `auradb_raft_commit_index`, `auradb_raft_applied_index`,
  `auradb_raft_log_last_index`, `auradb_raft_leader_changes_total`,
  `auradb_raft_votes_granted_total`, `auradb_raft_append_entries_sent_total`,
  `auradb_raft_append_entries_received_total`,
  `auradb_raft_replication_lag_entries`, `auradb_replication_apply_errors_total`,
  and the `auradb_raft_apply_latency_us` summary. See
  [REPLICATION.md](REPLICATION.md).
- **Multi-node preview metrics (v0.5.0, present when the preview is enabled)** -
  `auradb_peer_connected`, `auradb_peer_replication_lag_entries`,
  `auradb_raft_elections_total`, `auradb_raft_election_timeouts_total`,
  `auradb_raft_append_entries_failures_total`,
  `auradb_raft_heartbeat_latency_ms`, and `auradb_cluster_quorum_available`.
  These cover peer connectivity, per-peer replication lag, election activity,
  AppendEntries failures, heartbeat latency, and whether a majority is available.
  See [CLUSTERING.md](CLUSTERING.md).

A `snapshot()` is serializable and can be exported:

- `render_prometheus()` - minimal Prometheus text exposition format.
- `to_json()` - JSON.

The server updates counters and gauges as it handles requests, opens/closes
cursors and transactions, and reads/writes bytes.

## MVCC and GC

Engine statistics also report the **total stored versions** across all record
chains and the **active transaction count** (the number of live snapshots), which
together indicate how much version history GC may be holding for in-flight
readers. Background version GC logs each pass with the number of versions and
records it reclaimed. See [STORAGE_ENGINE.md](STORAGE_ENGINE.md) and the `[mvcc]`
section of [CONFIGURATION.md](CONFIGURATION.md).

## Health and readiness

The `Health` opcode returns a `HealthReport { status, ready, version,
collections, mvcc }`. The CLI `status` command uses it. Readiness is true once the
engine has opened successfully. The additive `mvcc` section (AWP 1, optional)
reports active transactions, timed-out transactions, the oldest active read
timestamp and snapshot age, retained versions, cumulative transaction timeouts,
the configured transaction timeout, and whether GC is enabled — so an operator can
watch version pressure without scraping Prometheus. Older clients ignore the
field.

`auradb doctor` raises operational **warnings** when the active transaction count
is high, the oldest snapshot is too old, retained versions exceed a threshold, GC
is disabled, transaction timeouts are disabled, statistics are stale, or the index
consistency check fails. See [OPERATIONS.md](OPERATIONS.md).

### Cluster health (v0.4.0, extended in v0.5.0 and v0.5.1)

When cluster mode is enabled, the health report gains an additive `cluster`
section: `node_id`, `cluster_id`, `role`, `term`, `leader_id`, `commit_index`,
`applied_index`, `last_log_index`, `peer_count`, `single_node`, and
`replication_lag_entries`. New in v0.5.0, the section also carries:

- `preview_multi_node` (bool) — whether the experimental multi-node preview is
  active.
- `quorum_available` (bool) — whether a majority of nodes is connected (a
  minority cannot commit).
- `peers` — an array of one entry per declared peer. New in v0.5.1, each entry is
  `{ node_id, addr, client_addr?, connected, connect_attempts, match_index,
  next_index }`.
- `leader_client_addr` (v0.5.1) — the recognized leader's client-facing address
  when a peer declared a `client_addr`; omitted (unknown) otherwise.

These are additive AWP fields; the Aura Wire Protocol version is unchanged at AWP
1, and an older client ignores them. The `auradb status --json`, `auradb cluster
status --addr --json`, and `auradb doctor` outputs include the cluster fields.
The error payload also gained an additive, optional `retryable` hint (set for
`not_leader`, conflicts, and transaction timeouts). See
[CLUSTERING.md](CLUSTERING.md).

### JSON output

- `auradb status --json` connects to a running server and emits the address,
  reachability, status, readiness, server version, protocol version, collection
  count, and whether TLS was used.
- `auradb doctor --json` inspects a local data directory and emits the version,
  protocol version, data directory, storage/catalog/index status, the index load
  report (loaded versus rebuilt), the consistency result, and a redacted security
  summary (bind, public-bind flag, auth status, TLS status, mutual-TLS and
  insecure-bind flags).

Both redact secrets: the token hash and certificate or key material are never
included. The `status` JSON carries the fields the health frame exposes plus the
client-known protocol version; richer server-runtime counters (active
connections, transactions, and cursors) are available through the metrics
registry above.

## Roadmap

OpenTelemetry export and per-query-fingerprint metrics are future work; the
metrics registry and tracing here cover the first-release requirements without
requiring a collector.
