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
