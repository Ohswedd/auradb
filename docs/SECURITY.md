# Security

This document describes the implemented security posture of AuraDB `0.2.0`. See
`SECURITY.md` at the repository root for the vulnerability reporting policy.

AuraDB is a single-node server. It does not claim production-grade guarantees
beyond the controls described here, and it is honest about what is not
implemented (see [Not implemented](#not-implemented-do-not-rely-on)).

## Authentication

Static-token authentication is implemented and enforced when enabled.

- **Config.** The `[auth]` block has `enabled` (default `false`),
  `mode = "static-token"` (the only mode), `token_hash` (an Argon2id PHC
  string), and `token_hash_algorithm = "argon2id"` (the only algorithm).
- **Hashing.** Tokens are never stored in plaintext. `token_hash` holds an
  Argon2id PHC hash (`$argon2id$...`). Verification uses Argon2's constant-time
  comparison.
- **Generating a hash.** Run `auradb auth hash-token --token "secret"`, or omit
  `--token` to be prompted without echo. Paste the printed `$argon2id$...`
  string into `token_hash`.

```toml
[auth]
enabled = true
mode = "static-token"
token_hash = "$argon2id$v=19$m=19456,t=2,p=1$...$..."
token_hash_algorithm = "argon2id"
```

### Enforcement

When `auth.enabled = true`, clients must authenticate before any schema, query,
mutation, cursor, explain, migration-estimate, or transaction operation. Only
the HELLO handshake, AUTH, PING (liveness), and HEALTH (readiness) are allowed
unauthenticated.

A client may authenticate two ways:

1. **Handshake fast path.** Send `auth_token` in the HELLO handshake payload.
2. **Dedicated AUTH frame.** Send an AUTH frame (opcode `0x04`) with
   `{"token": "..."}`; the server replies `{"authenticated": true}` or an error.

The protocol adds `unauthenticated` and `invalid_credentials` error codes and
the AUTH (`0x04`) / AUTH_RESULT (`0x84`) opcodes. Failed attempts increment the
`auradb_auth_failures_total` metric. Secrets are never logged or echoed in error
frames.

### Fail closed

If `auth.enabled = true` but `token_hash` is missing, the server refuses to
start. A malformed `token_hash` also fails validation.

## TLS

Server-terminated TLS is implemented with rustls.

- **Config.** The `[tls]` block has `enabled`, `cert_path`, `key_path`,
  `client_ca_path`, and `require_client_cert` (mutual TLS).
- **Termination.** When `enabled = true`, the server terminates TLS itself.
  Missing or invalid certificate or key material aborts startup; plaintext is
  never served under a TLS configuration.
- **Mutual TLS.** Set `require_client_cert = true` with a `client_ca_path`
  bundle to reject clients that do not present a trusted certificate. Setting
  `require_client_cert = true` without `client_ca_path` fails startup.

```toml
[tls]
enabled = true
cert_path = ".local/certs/server.crt"
key_path  = ".local/certs/server.key"
# Mutual TLS:
# client_ca_path = ".local/certs/ca.crt"
# require_client_cert = true
```

### Development certificates

Generate development-only certificates with:

```bash
auradb cert generate-dev --out-dir .local/certs
```

This writes `ca.crt`, `ca.key`, `server.crt`, and `server.key`. The server
certificate has SANs `localhost` and `127.0.0.1` and is signed by the generated
development CA. These are clearly labeled development-only and must not be used in
production.

Clients trust the CA via `--tls-ca`:

```bash
auradb status --addr 127.0.0.1:7171 --tls-ca .local/certs/ca.crt --token secret
```

For Docker, mount certificates into the container and reference them in the
config.

## Secure bind defaults

- Binding `127.0.0.1` (loopback) is local developer mode; auth may be disabled.
- Binding a non-loopback address (for example `0.0.0.0`) with auth disabled is
  **rejected at startup** unless `allow_insecure_bind = true` is set in config or
  `--allow-insecure-bind` is passed to `auradb server`. This prevents
  accidentally exposing an unauthenticated server on a public interface.

## Redaction

`auradb doctor` prints a redacted security summary (bind address, whether the
bind is public, auth status, and TLS status). It never prints the token hash or
any other secret.

## Other implemented controls

- **Payload limits.** The server rejects any frame whose declared payload
  exceeds `max_payload_bytes`, before reading the body.
- **Frame validation.** Magic bytes, protocol version, header length, and CRC32
  header/payload checksums are validated before a frame is dispatched. Malformed
  frames produce a structured error and the connection is closed.
- **Fail-closed storage.** A checksum mismatch on a fully written batch is
  reported as corruption rather than silently dropped; a torn trailing batch is
  safely truncated on recovery. Corrupt or missing index snapshots are detected
  and the index is rebuilt from storage.
- **Safe Rust.** Every crate sets `#![forbid(unsafe_code)]`.
- **Safe file paths.** The engine writes only within the configured data
  directory; the manifest, catalog, and index snapshots are written via
  temp-file + rename.
- **Structured errors.** Errors carry stable codes and never leak internal paths
  or secrets beyond the message text.
- **Property/fuzz tests.** The frame decoder is property-tested against arbitrary
  and single-bit-corrupted input and must never panic
  (`crates/auradb-protocol/tests/fuzz.rs`). Deterministic seeded recovery tests
  exercise corruption detection and repair (see [TESTING](TESTING.md)).

## Not implemented (do not rely on)

AuraDB is single node. The following are not implemented in `0.2.0`:
role-based access control (RBAC/ABAC), tenant isolation, field-level read/write
policies, field-level encryption, encryption at rest, and audit logging. There
is no clustering, replication, sharding, or Raft. The roadmap is in
[ROADMAP](ROADMAP.md).

## Operational guidance

Enable authentication and TLS for any deployment reachable beyond the local
host. Keep the server single node, behind your own network controls, and treat
the development certificates as strictly local. For controls not yet
implemented (RBAC, encryption at rest, audit logging), front AuraDB with
infrastructure that provides them until they land.
