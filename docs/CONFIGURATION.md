# Configuration

The server is configured by a TOML file (see `AuraDB.toml`) with sensible
defaults; CLI flags override file values.

## Top-level options

| Key | Default | Description |
|---|---|---|
| `bind` | `127.0.0.1` | Bind address |
| `port` | `7171` | Listen port |
| `data_dir` | `.local/auradb` | Storage directory (segments, manifest, catalog, persisted indexes) |
| `max_payload_bytes` | `16777216` | Max accepted frame payload (16 MiB) |
| `log_level` | `info` | Tracing env-filter directive |
| `log_json` | `false` | Emit JSON logs |
| `cursor_timeout_secs` | `300` | Idle cursor timeout |
| `page_size` | `100` | Rows per query page before a cursor is used |
| `sync_on_commit` | `true` | fsync the log after each commit |
| `metrics_enabled` | `true` | Enable metrics collection |
| `allow_insecure_bind` | `false` | Permit a non-loopback bind while auth is disabled |
| `[tls]` | disabled | Server-terminated TLS (see below) |
| `[auth]` | disabled | Static-token authentication (see below) |

### Secure bind

Binding `127.0.0.1` (loopback) is local developer mode and may leave auth
disabled. Binding a non-loopback address (for example `0.0.0.0`) with auth
disabled is rejected at startup unless `allow_insecure_bind = true` is set here
or `--allow-insecure-bind` is passed to `auradb server`.

## `[auth]`

Static-token authentication, enforced when enabled. See
[SECURITY.md](SECURITY.md) for the full model.

```toml
[auth]
enabled = false                  # true enforces auth on all data operations
mode = "static-token"            # only supported mode
# token_hash = "$argon2id$v=19$m=19456,t=2,p=1$...$..."
token_hash_algorithm = "argon2id"  # only supported algorithm
```

- `token_hash` is an Argon2id PHC string, never a plaintext token. Generate it
  with `auradb auth hash-token`.
- When `enabled = true`, a missing or malformed `token_hash` fails startup.

## `[tls]`

Server-terminated TLS with rustls, including optional mutual TLS. See
[SECURITY.md](SECURITY.md).

```toml
[tls]
enabled = false              # true terminates TLS; missing/invalid material fails startup
# cert_path = "server.crt"
# key_path = "server.key"
# client_ca_path = "ca.crt"        # CA bundle for verifying client certificates
# require_client_cert = false      # mutual TLS; requires client_ca_path
```

- When `enabled = true`, plaintext is never served. Missing or invalid
  certificate or key material aborts startup.
- `require_client_cert = true` without `client_ca_path` fails startup.
- Generate development-only certificates with `auradb cert generate-dev`.

## Loading and overrides

```bash
auradb server --config AuraDB.toml --data-dir /var/lib/auradb --bind 0.0.0.0 --port 7171
```

`Config::load` parses the file; missing keys fall back to defaults. Validation
runs before the server starts and **fails closed** on invalid values (zero port,
zero payload limit, zero page size), on unsafe configuration (a public bind
without auth and without `allow_insecure_bind`), and on incomplete security
material (auth enabled without a valid `token_hash`, TLS enabled without valid
certificate/key, mutual TLS without a client CA). Validate a config without
starting the server with `auradb config validate --config AuraDB.toml`.

## Docker

The image reads the same flags; `docker-compose.yml` binds `0.0.0.0:7171` inside
the container and mounts `/data` as a volume. To enable TLS, mount certificates
into the container and reference them under `[tls]`.
