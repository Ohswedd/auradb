# Security Policy

## Supported versions

AuraDB is at `0.3.1`. Security fixes target the latest released version.

## Reporting a vulnerability

Please report suspected vulnerabilities privately through GitHub's private
vulnerability reporting (the repository's Security tab) rather than opening a
public issue. Include reproduction steps and affected versions. We aim to
acknowledge reports within a few business days.

## Security posture of this release

AuraDB `0.3.1` is a single-node engine. Its implemented and enforced security
controls are:

- **Authentication.** Optional static-token authentication, enforced when
  enabled. Tokens are verified against an Argon2id hash with constant-time
  comparison and are never stored or logged in plaintext.
- **TLS.** Optional server-terminated TLS (rustls), including mutual TLS.
  Fail-closed: missing or invalid certificate or key material aborts startup, and
  plaintext is never served under a TLS configuration.
- **Safe bind defaults.** A non-loopback bind with authentication disabled is
  rejected at startup unless explicitly overridden (`--allow-insecure-bind`),
  which is for local development only.
- **Payload limits.** The server rejects frames whose declared payload exceeds
  `max_payload_bytes` before reading the body.
- **Transaction resource guardrails.** Idle transactions are reaped after
  `[mvcc] transaction_timeout_secs`, releasing their pinned MVCC snapshots, and a
  closed connection's transactions are rolled back, so an abandoned or dropped
  transaction cannot pin versions indefinitely. MVCC pressure is exposed through
  metrics and `auradb doctor` warnings.
- **Frame validation.** Magic bytes, protocol version, header length, and CRC32
  header and payload checksums are validated before a frame is processed;
  malformed frames yield a structured error and the connection is closed.
- **Fail-closed storage.** Checksum failures on fully written batches are
  reported as corruption rather than silently dropped; a torn trailing batch is
  safely truncated on recovery.
- **Safe Rust.** `#![forbid(unsafe_code)]` is enabled across every crate.
- **Property and fuzz tests.** The frame decoder is property-tested against
  arbitrary and corrupted input (`crates/auradb-protocol/tests/fuzz.rs`).

## Not implemented (do not rely on)

AuraDB is single node. Role-based access control (RBAC), tenant isolation,
field-level encryption, encryption at rest, and audit logging are not
implemented. There is no clustering, replication, sharding, or Raft.

## Operational guidance

Enable authentication and TLS for any deployment reachable beyond the local host;
the recommended path is the secure Docker Compose example
(`docker-compose.secure.yml`). Treat development certificates from
`auradb cert generate-dev` as strictly local, and inject secrets from a managed
store in production. See [`docs/SECURITY.md`](docs/SECURITY.md) for the full
security model, [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md) for deployment, and
[`docs/ROADMAP.md`](docs/ROADMAP.md) for the roadmap.
