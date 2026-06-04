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
- **Gauges** - `active_connections`, `active_transactions`, `active_cursors`.
- **Histograms** - `request_latency`, `query_latency`, `storage_latency`
  (fixed microsecond buckets with sum and count).

A `snapshot()` is serializable and can be exported:

- `render_prometheus()` - minimal Prometheus text exposition format.
- `to_json()` - JSON.

The server updates counters and gauges as it handles requests, opens/closes
cursors and transactions, and reads/writes bytes.

## Health and readiness

The `Health` opcode returns a `HealthReport { status, ready, version,
collections }`. The CLI `status` command uses it. Readiness is true once the
engine has opened successfully.

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
