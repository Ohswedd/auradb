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

## Roadmap

OpenTelemetry export and per-query-fingerprint metrics are future work; the
metrics registry and tracing here cover the first-release requirements without
requiring a collector.
