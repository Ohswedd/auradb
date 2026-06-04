# Configuration

The server is configured by a TOML file (see `AuraDB.toml`) with sensible
defaults; CLI flags override file values.

## Options

| Key | Default | Description |
|---|---|---|
| `bind` | `127.0.0.1` | Bind address |
| `port` | `7171` | Listen port |
| `data_dir` | `.local/auradb` | Storage directory |
| `max_payload_bytes` | `16777216` | Max accepted frame payload (16 MiB) |
| `log_level` | `info` | Tracing env-filter directive |
| `log_json` | `false` | Emit JSON logs |
| `cursor_timeout_secs` | `300` | Idle cursor timeout |
| `page_size` | `100` | Rows per query page before a cursor is used |
| `sync_on_commit` | `true` | fsync the log after each commit |
| `metrics_enabled` | `true` | Enable metrics collection |
| `[tls]` | disabled | TLS shape (see below) |
| `[auth]` | disabled | Static-token auth shape (see below) |

## Loading and overrides

```bash
auradb server --config AuraDB.toml --data-dir /var/lib/auradb --bind 0.0.0.0 --port 7171
```

`Config::load` parses the file; missing keys fall back to defaults. `validate()`
runs before the server starts and **fails closed** on invalid values (zero port,
zero payload limit, zero page size) and on unsupported requests.

## TLS and auth (shapes only)

```toml
[tls]
enabled = false        # true fails closed - TLS is not implemented; use a proxy

[auth]
required = false
static_tokens = []
```

These shapes exist for forward compatibility. Neither TLS termination nor
authentication is enforced in this release; setting `tls.enabled = true` makes
the server refuse to start rather than serve plaintext. See `docs/SECURITY.md`.

## Docker

The image reads the same flags; `docker-compose.yml` binds `0.0.0.0:7171` inside
the container and mounts `/data` as a volume.
