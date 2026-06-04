# Security

This document describes the implemented security posture of AuraDB `0.1.0`. See
`SECURITY.md` at the repository root for the vulnerability reporting policy.

## Implemented controls

- **Payload limits.** The server rejects any frame whose declared payload
  exceeds `max_payload_bytes`, before reading the body.
- **Frame validation.** Magic bytes, protocol version, header length, and CRC32
  header/payload checksums are validated before a frame is dispatched. Malformed
  frames produce a structured error and the connection is closed.
- **Fail-closed storage.** A checksum mismatch on a fully written batch is
  reported as corruption rather than silently dropped; a torn trailing batch is
  safely truncated on recovery.
- **Safe Rust.** Every crate sets `#![forbid(unsafe_code)]`.
- **Safe file paths.** The engine writes only within the configured data
  directory; the manifest and catalog are written via temp-file + rename.
- **Structured errors.** Errors carry stable codes and never leak internal
  paths or secrets beyond the message text.
- **Property/fuzz tests.** The frame decoder is property-tested against arbitrary
  and single-bit-corrupted input and must never panic
  (`crates/auradb-protocol/tests/fuzz.rs`).

## Not implemented (do not rely on)

- **TLS.** The config exposes a `[tls]` shape; `enabled = true` **fails closed**
  (the server refuses to start). Terminate TLS with a proxy in front of AuraDB.
- **Authentication / authorization.** A `[auth]` static-token shape exists but is
  **not enforced**. RBAC/ABAC, tenant isolation, field-level read/write policies,
  field-level encryption, encryption at rest, and audit logging are not
  implemented.

## Operational guidance

Run AuraDB `0.1.0` only on a trusted network, or behind a TLS-terminating,
authenticating proxy, until the controls above land. The roadmap is in
[ROADMAP](ROADMAP.md).
