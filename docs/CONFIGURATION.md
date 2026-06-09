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
| `max_payload_bytes` | `16777216` | Max accepted wire frame payload (16 MiB) |
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
| `[limits]` | enabled | Enforced resource bounds on requests (see below) |

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

## `[limits]`

Enforced, configurable resource bounds on incoming requests, added in v0.8.0. Each
bound is validated **non-zero at startup** (a zero value fails closed). When a
request would exceed one of these limits the server returns a structured
`limit_exceeded` error and **keeps the connection open** — the limit rejects the
offending operation, not the session.

```toml
[limits]
max_query_limit = 1000000             # max rows a single query may request
max_full_text_query_tokens = 64       # max tokens in a contains_text query
max_document_depth = 64               # max nesting depth of a document value
max_vector_dimension = 4096           # max declared/queried vector dimension
max_transaction_write_set = 100000    # max staged writes in one transaction
max_query_time_ms = 30000             # default read deadline; 0 disables (v1.2.0)
```

| Key | Default | Bound |
|---|---|---|
| `max_query_limit` | `1000000` | Maximum `limit` accepted on a query |
| `max_full_text_query_tokens` | `64` | Maximum tokens in a `contains_text` query |
| `max_document_depth` | `64` | Maximum nesting depth of a document value |
| `max_vector_dimension` | `4096` | Maximum declared or queried vector dimension |
| `max_transaction_write_set` | `100000` | Maximum staged writes in a single transaction |
| `max_query_time_ms` | `30000` | Default per-read execution deadline in ms; `0` disables it (v1.2.0) |

`max_query_time_ms` is the cooperative default deadline for every read (find,
count, exists, aggregate/facet). A request may carry a per-query `timeout_ms` that
**lowers** this bound but never raises it; a read that exceeds the effective
deadline returns a structured `query_timeout` error without tearing down the
connection. Set `0` to disable the default for workloads with intentionally long
reads.

Each bound is **inclusive** (a request exactly at the limit is accepted; one past
it is refused) and carries **no upper cap** — these are operator policy, not
hard-coded ceilings, so a deliberately high value validates and an operator who
sets one owns the trade-off. A zero value is rejected at startup. When a
`max_document_depth` violation is reported, the error **names the offending
top-level field** so the over-nested path is easy to find; the field name is
structural, not record content.

These bounds are independent of `max_payload_bytes`, which is the **wire frame**
bound (the largest accepted AWP frame payload, 16 MiB) enforced at the transport
layer. The CLI `restore` command additionally enforces a 64 MiB per-line bound on
its input JSONL. See [`examples/auradb.toml`](../examples/auradb.toml) for the
documented section.

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

## `[cluster]`

Cluster (Raft) mode, added in v0.4.0 and extended with an experimental
cross-process multi-node preview in v0.5.0. **Disabled by default**; when disabled
the whole table is inert and the engine uses the single-node direct write path
exactly as in v0.3.1.

> **AuraDB v0.5.0 introduces a controlled, experimental multi-node server
> preview. Single-node mode remains the recommended production mode.**

> **v0.6.2 adds no new configuration.** The repeated-chaos and larger-state
> recovery hardening is tests and diagnostics only; the `[cluster]` options below
> are unchanged, and the recommended production mode is still single-node.

A single-node cluster (no peers):

```toml
[cluster]
enabled = true                  # enable cluster (Raft) mode
cluster_id = ""                 # optional pinned cluster id (32 hex); empty = use/generate
node_id = ""                    # optional pinned node id (16 hex, non-zero); empty = use/generate
listen_addr = "127.0.0.1:7172"  # cluster (Raft) transport bind
advertise_addr = "127.0.0.1:7172"  # address advertised to peers (may differ behind NAT)
bootstrap = true                # bootstrap a brand-new cluster
peers = []                      # empty: single-node cluster
```

A three-node loopback preview node (no TLS — the validated local path; the other
nodes mirror this with their own `node_id`/ports):

```toml
[cluster]
enabled = true
experimental_multi_node = true   # second opt-in, required for a real cluster
cluster_id = "0000000000000000000000000000a1a2"  # identical on every node
node_id    = "00000000000000a1"                  # distinct per node
listen_addr    = "127.0.0.1:7172"
advertise_addr = "127.0.0.1:7172"
# This node's own client-facing address (v0.9.1). While this node is the leader it
# reports this in the `not_leader` hint and in cluster status/health. It should
# match the client_addr the other nodes list for this node below.
advertise_client_addr = "127.0.0.1:7171"
bootstrap = true
# Static membership: every other node, by id and cluster address. The optional
# client_addr (v0.5.1) lets a `not_leader` response and the cluster diagnostics
# report the leader's client address so a client can redirect.
peers = [
  { node_id = "00000000000000a2", addr = "127.0.0.1:7182", client_addr = "127.0.0.1:7181" },
  { node_id = "00000000000000a3", addr = "127.0.0.1:7192", client_addr = "127.0.0.1:7191" },
]
```

Each peer entry takes `node_id` and `addr` (the cluster/Raft transport address),
plus an optional `client_addr` (the peer's client-facing `host:port`). When
`client_addr` is set, a `not_leader` error and `auradb cluster status --addr`
report the leader's client address; when omitted, that address is reported as
unknown rather than guessed.

A node never appears in its own `peers` list, so the optional `advertise_client_addr`
field **(v0.9.1)** lets a node declare its **own** client-facing `host:port`. While
this node is the leader it reports that address in the `not_leader` hint and in
cluster status/health, so a client that queries the leader directly still learns
its client address instead of an empty value. The field is operator-declared and
honest: it is never guessed and is never the peer/Raft transport address. It
should match the `client_addr` the other nodes list for this node. When it is
omitted the leader simply reports its own client address as unknown and clients
fall back to re-resolving the leader. The field is additive and backward
compatible — a config that omits it behaves exactly as before v0.9.1.

A public (non-loopback) preview additionally requires
`allow_experimental_public_cluster = true`, peer TLS, and a token:

```toml
[cluster]
enabled = true
experimental_multi_node = true
allow_experimental_public_cluster = true
peer_auth_token = "a-shared-secret"   # verified in the PeerHello handshake
# ... cluster_id / node_id / addrs / peers ...

[cluster.tls]
enabled   = true
cert_path = "/certs/peer.crt"
key_path  = "/certs/peer.key"
ca_path   = "/certs/ca.crt"
```

Fields:

- `enabled` — when `true` with no `peers`, the node runs as a real, durable
  single-node cluster: every commit is ordered through the Raft log. A single-node
  cluster provides no fault tolerance.
- `experimental_multi_node` — **(v0.5.0)** the second opt-in. A non-empty `peers`
  list **without** this set to `true` is rejected at startup (preserving v0.4.1
  behavior), so a cluster never silently forms.
- `allow_experimental_public_cluster` — **(v0.5.0)** permit a non-loopback
  cluster address. Any non-loopback listen/advertise/peer address is rejected
  unless this is `true`, and setting it **additionally requires** `[cluster.tls]`
  and a `peer_auth_token`.
- `cluster_id` / `node_id` — leave empty to use the persisted identity (created by
  `auradb init` / `auradb cluster init`) or generate one. In a real cluster
  `cluster_id` is identical on every node and each `node_id` is distinct. Pinned
  ids are enforced; a mismatch with the persisted id is rejected.
- `listen_addr` / `advertise_addr` — the dedicated peer (Raft) transport. A
  non-loopback address fails closed unless `allow_experimental_public_cluster`.
- `advertise_client_addr` — **(v0.9.1)** optional; this node's own client-facing
  `host:port`. While this node is the leader it is reported as the leader client
  address in the `not_leader` hint and in cluster status/health. It is
  operator-declared (never guessed, never the transport address), should match the
  `client_addr` peers list for this node, and is omitted from the report when
  unset. Additive and backward compatible.
- `peer_auth_token` — **(v0.5.0)** shared token verified in the `PeerHello`
  handshake; required for a public cluster. It is treated as a secret and never
  printed or logged.
- `peers` — **(v0.5.0)** static membership as `{ node_id, addr }` entries (inline
  or `[[cluster.peers]]`). There is no join/leave/dynamic membership. A duplicate
  peer, a self-peer, or a malformed `host:port` is rejected.
- `[cluster.tls]` — **(v0.5.0)** peer-transport TLS (`cert_path`, `key_path`,
  `ca_path`); required for a public cluster.

Validate a cluster configuration offline with
`auradb config validate --config AuraDB.toml` or `auradb cluster doctor`. A
single-node example ships at `examples/auradb.cluster.local.toml`; the three-node
loopback preview at `examples/cluster/node{1,2,3}.toml`; and the Docker Compose
preview (peer TLS + token) at `examples/cluster/docker/`. See
[CLUSTERING.md](CLUSTERING.md), [SECURITY.md](SECURITY.md),
[CLUSTER_TROUBLESHOOTING.md](CLUSTER_TROUBLESHOOTING.md), and [CLI.md](CLI.md).
