# AuraDB 0.2.0 Release Notes

Released 2026-06-04.

AuraDB 0.2.0 is a single-node release themed around security, durability
hardening, and public usability. It keeps the wire protocol from 0.1.0 backward
compatible while adding enforced authentication, server-terminated TLS, durable
index snapshots, document-path and full-text search, and a published Docker image
plus prebuilt binaries.

## Highlights

- **Enforced authentication.** Optional static-token auth, verified against an
  Argon2id hash with constant-time comparison. Tokens are never stored in
  plaintext.
- **Server-terminated TLS.** Optional rustls TLS, including mutual TLS. Both auth
  and TLS fail closed.
- **Persisted indexes.** Indexes are snapshotted to disk at checkpoints and
  loaded on open when they are provably current, otherwise safely rebuilt.
- **Document-path and full-text search.** Equality acceleration on nested
  document values, and a basic tokenized full-text index.
- **Public usability.** A published Docker image, prebuilt binaries for five
  targets with checksums, a compatibility matrix, and new CLI tooling.

## Security

### Authentication

Enable static-token authentication with the `[auth]` config block. Generate a
hash with the CLI and paste it into `token_hash`:

```bash
auradb auth hash-token --token "your-secret"
```

```toml
[auth]
enabled = true
mode = "static-token"
token_hash = "$argon2id$v=19$m=19456,t=2,p=1$...$..."
token_hash_algorithm = "argon2id"
```

When enabled, clients must authenticate before any schema, query, mutation,
cursor, explain, migration-estimate, or transaction operation. Only HELLO, AUTH,
PING, and HEALTH are allowed unauthenticated. Clients authenticate by sending
`auth_token` in the HELLO handshake or by sending a dedicated AUTH frame. Failed
attempts increment `auradb_auth_failures_total`, and secrets are never logged or
echoed in error frames. A misconfigured auth block (enabled without a valid
`token_hash`) fails startup.

### TLS

Enable server-terminated TLS with the `[tls]` config block, and generate
development-only certificates with the CLI:

```bash
auradb cert generate-dev --out-dir .local/certs
```

```toml
[tls]
enabled = true
cert_path = ".local/certs/server.crt"
key_path  = ".local/certs/server.key"
# Mutual TLS:
# client_ca_path = ".local/certs/ca.crt"
# require_client_cert = true
```

Missing or invalid certificate or key material aborts startup, so plaintext is
never served under a TLS configuration. Mutual TLS rejects clients that do not
present a trusted certificate, and `require_client_cert = true` without a
`client_ca_path` fails startup.

### Secure defaults

Binding a non-loopback address (for example `0.0.0.0`) with auth disabled is
rejected at startup unless you explicitly opt in with `allow_insecure_bind = true`
or `--allow-insecure-bind`. `auradb doctor` prints a redacted security summary
and never reveals the token hash. See [SECURITY.md](SECURITY.md).

## Durability

### Persisted index snapshots

Indexes are snapshotted into an `indexes/` directory (an `INDEX_MANIFEST.json`
and per-collection framed, CRC32-checked `.idx` files) at checkpoints:
`auradb compact`, graceful shutdown, and `auradb index rebuild`. On open, a
snapshot loads only when its content fingerprint, schema field shape, and CRC all
match the current storage state; otherwise the engine rebuilds from the durable
record log and records that it rebuilt. A crash between checkpoints is detected as
a fingerprint mismatch and triggers a rebuild, so queries never return incorrect
results from a stale snapshot. Inspect this with `auradb index check` and force a
fresh snapshot with `auradb index rebuild`. See [INDEXING.md](INDEXING.md) and
[STORAGE_ENGINE.md](STORAGE_ENGINE.md).

### Recovery testing

This release adds deterministic, fixed-seed randomized recovery tests (never
flaky). They cover random insert/update/delete sequences verified against a
reference model after restart (with and without a checkpoint), trailing-segment
truncation recovery, mid-batch byte-flip corruption detection, catalog corruption
detection (fail closed), corrupt or missing index file repair, and corrupt index
manifest repair. They live in `crates/auradb-storage/tests/recovery.rs` and
`crates/auradb/tests/recovery.rs`. See [TESTING.md](TESTING.md).

## Search

### Document-path indexes

Declare a document-path index in a schema to accelerate equality filters on a
nested document value addressed by a dotted path:

```json
{ "indexes": [ { "path": "profile.company", "kind": "document_path" } ] }
```

The planner uses it for equality and reports it in EXPLAIN
(`strategy: index_lookup`, `used_index: "profile.company"`). It is maintained on
update and delete and persisted across restart. A document-path index indexes the
value at the exact path; indexing individual elements of an array at a path is not
supported. See [DOCUMENTS.md](DOCUMENTS.md).

### Full-text search

Declare a full-text index on a string field:

```json
{ "indexes": [ { "path": "body", "kind": "full_text" } ] }
```

Query it with a `contains_text` filter. The tokenizer lowercases text and splits
on every non-alphanumeric boundary, with no stop-word removal. Matching is boolean
AND (a record must contain every distinct query token), and results are ranked by
summed term frequency. This is honest, basic full-text search; it is **not** BM25.
Without a full-text index on the field, `contains_text` falls back to a tokenized
full scan with identical semantics. See [FULL_TEXT.md](FULL_TEXT.md).

## Public usability

### Docker

A published image is available on the GitHub Container Registry:

```bash
docker run --rm -p 7171:7171 -v auradb-data:/data ghcr.io/ohswedd/auradb:0.2.0
```

It runs as a non-root user, exposes `7171`, stores data in the `/data` volume,
and ships a `HEALTHCHECK` that calls `auradb status`. To enable TLS, mount your
certificates into the container and reference them in the config.

### Prebuilt binaries

Tagged releases attach prebuilt archives for `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, and
`x86_64-pc-windows-msvc`. Each archive contains the `auradb` binary, the README,
the LICENSE, and an example config, and a `SHA256SUMS` file is attached for
verification.

### Compatibility matrix

AuraDB 0.2.0 speaks AWP 1, and the published Aura Connector 0.3.x ships a native
AuraDB-over-TCP backend that speaks AWP 1 including auth and TLS. See
[COMPATIBILITY.md](COMPATIBILITY.md) and
[AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md).

### CLI

New and updated commands: `auth hash-token`, `cert generate-dev`,
`config validate`, `compatibility`, `index check`, and `index rebuild`; `status`
gains `--token`, `--tls-ca`, and `--tls-server-name`; and `server` gains
`--allow-insecure-bind`. See [CLI.md](CLI.md).

## Upgrading from 0.1.0

- **Wire format unchanged.** The AWP framing (44-byte header, magic, version, and
  checksums) is unchanged and backward compatible. 0.2.0 only adds fields and
  opcodes (optional HELLO `auth_token`, `auth_required` / `authenticated` in the
  HELLO_ACK, AUTH / AUTH_RESULT opcodes, and the `unauthenticated` /
  `invalid_credentials` error codes).
- **New config shapes.** The `[auth]` and `[tls]` blocks now drive real,
  enforced behavior. They default to disabled, so an existing config keeps
  working. To enable auth, add a `token_hash` from `auradb auth hash-token`; to
  enable TLS, provide a certificate and key.
- **Public binds.** If you bind a non-loopback address with auth disabled, the
  server now refuses to start unless you set `allow_insecure_bind = true` or pass
  `--allow-insecure-bind`.
- **Matching client.** Use Aura Connector 0.3.x. The 0.2.x connector uses a
  different internal framing and is not wire compatible.

## Known limitations

AuraDB 0.2.0 is single node. It does not provide clustering, replication,
sharding, or Raft. Transactions are single-node optimistic (read-version conflict
detection), not serializable MVCC. Vector search is exact, not ANN/HNSW.
Full-text search is tokenized boolean-AND matching with term-frequency ranking,
not BM25. RBAC, field-level encryption, encryption at rest, and audit logging are
not implemented. See the [roadmap](ROADMAP.md) for direction.
