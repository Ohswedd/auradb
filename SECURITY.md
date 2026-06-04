# Security Policy

## Supported versions

AuraDB is at `0.1.0` (first single-node developer release). Security fixes target
the latest released version.

## Reporting a vulnerability

Please report suspected vulnerabilities privately to the maintainers rather than
opening a public issue. Include reproduction steps and affected versions. We aim
to acknowledge reports within a few business days.

## Security posture of this release

AuraDB `0.1.0` is a single-node engine. Its implemented security controls are:

- **Payload limits.** The server rejects frames whose declared payload exceeds
  `max_payload_bytes`.
- **Frame validation.** Magic bytes, protocol version, header length, and CRC32
  header and payload checksums are validated before a frame is processed;
  malformed frames yield a structured error and the connection is closed.
- **Fail-closed storage.** Checksum failures on fully written batches are
  reported as corruption rather than silently dropped.
- **Safe Rust.** `#![forbid(unsafe_code)]` is enabled across every crate.
- **Property and fuzz tests.** The frame decoder is property-tested against
  arbitrary and corrupted input (`crates/auradb-protocol/tests/fuzz.rs`).

## Not yet implemented (do not rely on)

The configuration exposes shapes for TLS and static-token authentication, but
neither is enforced in this release:

- `tls.enabled = true` fails closed (the server refuses to start) so plaintext is
  never served under a TLS configuration. Terminate TLS with a proxy in front of
  AuraDB.
- Authentication, authorization (RBAC), tenant isolation, field-level encryption,
  and audit logging are not implemented.

## Operational guidance

Run AuraDB only on trusted networks, or behind a TLS-terminating, authenticating
proxy, until these controls land. Do not expose it directly to untrusted networks
without those controls in place. See [`docs/SECURITY.md`](docs/SECURITY.md) for
details and [`docs/ROADMAP.md`](docs/ROADMAP.md) for the roadmap.
