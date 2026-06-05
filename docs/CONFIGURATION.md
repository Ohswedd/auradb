# Configuration

The server is configured by a TOML file (see `AuraDB.toml`) with sensible
defaults; CLI flags override file values.

Ready-to-use templates live under `examples/`:

- [`auradb.local.toml`](../examples/auradb.local.toml) - local development
  (loopback, auth and TLS disabled).
- [`auradb.secure.toml`](../examples/auradb.secure.toml) - secure deployment
  (auth and TLS enabled, redacted token-hash placeholder).
- [`auradb.toml`](../examples/auradb.toml) - a balanced default that documents
  every option.

See [DEPLOYMENT.md](DEPLOYMENT.md) for the secure Docker Compose example.

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
| `[mvcc]` | enabled | MVCC version garbage collection (see below) |

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
- Rotate the token in place with `auradb auth rotate-token --config <file>
  --token <new>`: it re-hashes the new token, rewrites the config atomically with
  unrelated fields preserved, optionally backs up the previous config, and
  validates the result. A running server keeps the token it loaded at startup;
  restart it to enforce the new token. See [SECURITY.md](SECURITY.md).

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

## `[mvcc]`

MVCC version garbage collection. AuraDB stores a chain of committed versions per
record; this section controls how old versions are reclaimed in the background.
See [STORAGE_ENGINE.md](STORAGE_ENGINE.md) and [TRANSACTIONS.md](TRANSACTIONS.md).

```toml
[mvcc]
gc_enabled = true             # run version GC in the background
gc_interval_secs = 300        # seconds between background GC passes
min_retained_versions = 1     # minimum versions kept per record chain
```

- When `gc_enabled = true`, the server runs version GC every `gc_interval_secs`,
  using the oldest active transaction snapshot (or the commit watermark) as the
  horizon so a version a live transaction can still observe is never reclaimed.
- `min_retained_versions` is the floor of versions kept per chain; the latest
  version is always retained regardless.
- GC can also be run on demand with `auradb gc`. See [CLI.md](CLI.md).

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
starting the server with `auradb config validate --config AuraDB.toml`. To
validate a deployment template whose TLS files live on the target host, add
`--no-file-checks`, which checks structure without requiring the certificate and
key to exist on the machine running the check; every other check still applies.

## Docker

The image reads the same flags. `docker-compose.yml` is a development example
that binds `0.0.0.0:7171` inside the container and mounts `/data` as a volume.
For a deployment, use `docker-compose.secure.yml`, which enables auth and TLS,
mounts a config and a certificate directory, and injects the token hash from the
environment so no secret is committed. See [DEPLOYMENT.md](DEPLOYMENT.md).

## MVCC and transaction lifecycle (`[mvcc]`)

```toml
[mvcc]
gc_enabled = true                       # run background version GC
gc_interval_secs = 300                  # seconds between GC passes
min_retained_versions = 1               # versions of each live record GC always keeps
transaction_timeout_secs = 300          # reap a transaction idle longer than this (0 = off)
abandoned_transaction_reaper_secs = 30  # how often the reaper runs
```

A transaction idle for longer than `transaction_timeout_secs` is reaped: it is
marked aborted, its MVCC snapshot is released so GC can progress, and any further
operation on it fails with a structured `transaction_timeout` error. The reaper
also releases transactions whose handle was dropped or whose connection vanished.

Setting `transaction_timeout_secs = 0` disables timeouts; this is not recommended,
because an abandoned transaction then pins versions indefinitely. Validation
rejects `abandoned_transaction_reaper_secs = 0` while timeouts are enabled. See
[OPERATIONS.md](OPERATIONS.md) and [TRANSACTIONS.md](TRANSACTIONS.md).
