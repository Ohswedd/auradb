# Security

This document describes the implemented security posture of AuraDB `1.0.1`. See
`SECURITY.md` at the repository root for the vulnerability reporting policy and
[SUPPORT_POLICY.md](SUPPORT_POLICY.md) for the v1.0 security support, supported
versions, backport, and accepted-advisory policy.

AuraDB is a single-node server. It does not claim production-grade guarantees
beyond the controls described here, and it is honest about what is not
implemented (see [Not implemented](#not-implemented-do-not-rely-on)).

**v1.0 security review.** For the v1.0 release line this posture was reviewed
against the production single-node deployment mode, and is re-confirmed each patch
(`cargo audit` / `cargo deny`): authentication and TLS are required for
network exposure (a public bind without auth is refused unless an explicit dev
override is set); client-token, peer-token, and certificate rotation are documented
below; secrets are redacted from `doctor` / `status` / `config validate` /
`check --json` / `backup verify` output; the Docker image and secure Compose file
run non-root; and `cargo audit` / `cargo deny` run in CI against a documented
advisory policy. Multi-node peer networking remains an HA candidate preview, off by
default, with the fail-closed baseline described below.

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

### Token rotation

Rotate the static token without hand-editing the config:

```bash
auradb auth rotate-token --config AuraDB.toml --token "new-secret" --backup
```

This re-hashes the new token with Argon2id, rewrites the config atomically
(temp-file plus rename) with unrelated fields preserved, optionally backs up the
previous config to `<config>.bak`, and re-reads and validates the written file.
The plaintext token is never stored or printed. A running server keeps the token
hash it loaded at startup, and connections authenticated with the old token stay
authenticated until they disconnect; AuraDB does not hot-reload the token, so
restart the server to enforce the new token.

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
config. The recommended deployment path is `docker-compose.secure.yml`, which
enables auth and TLS, runs as a non-root user, and injects the token hash from
the environment so no secret is committed. See [DEPLOYMENT.md](DEPLOYMENT.md).

## Secure bind defaults

- Binding `127.0.0.1` (loopback) is local developer mode; auth may be disabled.
- Binding a non-loopback address (for example `0.0.0.0`) with auth disabled is
  **rejected at startup** unless `allow_insecure_bind = true` is set in config or
  `--allow-insecure-bind` is passed to `auradb server`. This prevents
  accidentally exposing an unauthenticated server on a public interface.

## Cluster (peer) transport (v0.4.0, multi-node preview in v0.5.0, fail-stop recovery in v0.6.0)

> **AuraDB v0.6.0 improves the controlled multi-node preview and validates
> fail-stop recovery. It is _not_ production HA. Single-node mode remains the
> recommended production mode.** Peer certificate and token rotation (rolling
> restart, one node at a time) is documented below; the v0.6.0 peer snapshot
> install validates the cluster id, manifest digest, and storage format before
> touching follower state, so a wrong-cluster or tampered snapshot is rejected.

> **AuraDB v0.6.2 hardens repeated chaos and larger-state recovery in the
> controlled multi-node preview. It is _not_ production HA. Single-node mode
> remains the recommended production mode.** The peer-transport security baseline
> is **unchanged**: two opt-ins still gate the preview, membership is still static,
> and any non-loopback cluster address still requires peer TLS **and** a shared
> `peer_auth_token` (an unauthenticated public bind is refused). The v0.6.2
> network-interruption tests use a test-only, in-process transport partition
> control (`drop_peer_link` / `heal_peer_link`) that has **no configuration or CLI
> surface** and does not weaken the handshake, auth, or TLS checks.

Cluster (Raft) mode is disabled by default. v0.5.0 adds a real cross-process peer
transport, gated by a conservative, fail-closed security baseline:

- **Two opt-ins gate the preview.** A real cluster forms only with both `[cluster]
  enabled = true` and `experimental_multi_node = true`. A non-empty `peers` list
  without the second opt-in is rejected at startup (the v0.4.1 behavior).
- **Loopback is allowed without TLS for the local preview.** A loopback-only
  three-process cluster needs no peer TLS or token — this is the validated local
  path. The default `listen_addr` is `127.0.0.1:7172`.
- **Public clusters fail closed.** Any non-loopback cluster address (listen,
  advertise, or peer) is **rejected at startup** unless
  `allow_experimental_public_cluster = true`, which **additionally requires** peer
  TLS (`[cluster.tls]` with `cert_path` / `key_path` / `ca_path`) and a shared
  `peer_auth_token`.
- **Authenticated, identity-checked handshake.** Each peer connection opens with a
  `PeerHello` that verifies the protocol version, the cluster id, the peer's node
  id (against the static membership), and a shared token. A wrong-cluster,
  unknown-node, duplicate-node, or bad-token peer is rejected with a structured
  `PeerError`.
- **Frame hardening.** Each peer frame is magic-tagged (`APR1`),
  protocol-version-tagged (v1), length-delimited, and CRC32-checksummed, with a
  16 MiB payload-size limit; a frame failing any check is rejected.
- **Snapshot install is not implemented** and is answered with a structured
  *unsupported* response rather than being silently ignored.
- **Token redaction.** The `peer_auth_token` is treated like other AuraDB
  secrets: it is never printed by `doctor`/`status` output or logged.
- **Backup-plan redacts secrets (v0.6.1).** `auradb cluster backup-plan` references
  the auth token, peer auth token, and TLS material **redacted**, and notes they
  are **never written into a logical backup** (`auradb dump` exports data only).
- **Errors do not echo request payloads or record contents (v0.8.1).** A
  structured `limit_exceeded` error reports the violated bound and a count or
  field name, never the request payload — a secret embedded in a query or record
  cannot leak through a limit error. `auradb backup verify` likewise reports only
  collection names and counts; it never prints record field values, including a
  duplicated primary key it rejects.
- **Static membership only.** No join/leave/dynamic membership; a duplicate, a
  self-peer, or a malformed peer address is rejected.

### Development certificates and rotation (v0.5.1)

- **Generate per-node dev certificates.** `auradb cert generate-dev --out-dir DIR
  --server-name nodeN --san nodeN --san localhost --san 127.0.0.1` writes a CA
  (reused across calls in the same directory) plus a per-node certificate whose
  SAN matches the name peers dial. `examples/cluster/generate-dev-certs.sh` drives
  this for a three-node cluster. These certificates are **development only**.
- **Verify SANs.** A peer dialing `nodeN:7172` verifies the presented
  certificate's SAN against `nodeN`. A certificate from the **wrong CA** or with a
  **non-matching SAN** is rejected by the TLS handshake; an **expired** certificate
  is likewise rejected (validated by the `peer_tls` tests).
- **Rotate certificates with a rolling restart.** Re-issue a node's certificate
  from the same CA and restart only that node; peers trust the CA, not a specific
  leaf, so a freshly rotated certificate is accepted without disturbing the
  others. To rotate the CA itself, distribute a bundle of old+new CA in each
  node's `ca_path`, roll every node onto new-CA certificates, then drop the old CA
  in a second roll.
- **Rotate the peer token with a rolling restart.** Update `peer_auth_token` on
  each node and restart it; nodes still on the old token fail the handshake until
  updated, so keep a quorum and watch `auradb cluster status --addr`. Never commit
  `certs/`, private keys, or tokens — `examples/cluster/.gitignore` excludes them.

See [CLUSTERING.md](CLUSTERING.md), [OPERATIONS.md](OPERATIONS.md), and
[CONFIGURATION.md](CONFIGURATION.md).

## Redaction

`auradb doctor` prints a redacted security summary (bind address, whether the
bind is public, auth status, and TLS status). It never prints the token hash or
any other secret. The same applies to `auradb doctor --json` and `auradb status
--json`, and to `auradb auth rotate-token`, none of which include the token hash
or certificate or key material in their output.

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

## Security hardening checklist

Use this checklist when preparing a deployment. It consolidates the controls
described above; single-node mode is the recommended production mode.

- **Authentication enabled.** `[auth] enabled = true` with a real Argon2id
  `token_hash`. A public bind without auth is rejected at startup.
- **TLS enabled.** `[tls] enabled = true` with certificate and key material issued
  by your own CA (not the development certificates). Plaintext is never served
  under a TLS configuration.
- **Token rotation.** Rotate the client token in place with `auradb auth
  rotate-token` and restart to enforce it.
- **Peer token rotation.** In the multi-node preview, rotate `peer_auth_token` with
  a rolling restart, one node at a time, keeping a quorum.
- **Certificate rotation.** Re-issue per-node peer certificates from the shared CA
  and roll one node at a time; rotate the CA itself with an old+new bundle across
  two rolls.
- **No secrets in logs.** Tokens, token hashes, the peer token, and TLS material
  are never logged or echoed in error frames.
- **No token hash in diagnostics.** `auradb doctor`, `auradb status --json`,
  `auradb config validate`, and `auradb check --json` print a redacted summary and
  never include the token hash or any secret. This redaction is tested across
  `doctor`, `status`, `config validate`, and `check`.
- **Non-root Docker.** The published image and the secure compose file run as a
  non-root user.
- **Read-only root filesystem.** `docker-compose.secure.yml` runs with a read-only
  root filesystem.
- **Capability drop.** The secure compose file drops Linux capabilities it does not
  need.
- **No public bind without auth and TLS.** A non-loopback bind with auth disabled is
  rejected unless `allow_insecure_bind` is explicitly set (development only); a
  public cluster additionally requires peer TLS and a `peer_auth_token`.
- **Dependency audit.** `cargo audit` and `cargo deny` run in CI (`security.yml`)
  against a RUSTSEC advisory policy; keep dependencies current.
- **File permissions.** Restrict the data directory, config file, certificates, and
  private keys to the service account; never commit `certs/`, keys, or tokens.
- **Backup encryption.** Logical backups (`auradb dump`) are plaintext JSONL;
  encrypt them at rest and in transit with your own tooling.
- **Network exposure.** Keep AuraDB behind your own network controls; expose it
  beyond the local host only with auth and TLS enabled.

## Not implemented (do not rely on)

AuraDB is single node. The following are not implemented in `1.0.1`:
role-based access control (RBAC/ABAC), tenant isolation, field-level read/write
policies, field-level encryption, encryption at rest, and audit logging. The
recommended production deployment remains single-node. v0.5.0 adds a controlled,
experimental multi-node server preview (off by default, gated by two opt-ins,
with the peer-transport baseline described above), but it is not production-grade
peer networking: there is no automatic failover, no dynamic membership, no
streaming snapshot install, and no sharding. The roadmap is in
[ROADMAP](ROADMAP.md).

## Operational guidance

Enable authentication and TLS for any deployment reachable beyond the local
host. Keep the server single node, behind your own network controls, and treat
the development certificates from `auradb cert generate-dev` as strictly local.
For a production deployment, use certificates issued by your certificate
authority and inject the token hash from a managed secret store rather than a
plaintext file or shell variable. The `docker-compose.secure.yml` example reads
the token hash from the environment so no secret is committed; it was validated
at runtime with development certificates and a generated token hash (healthy over
TLS with auth, with no secret in the container logs). See
[DEPLOYMENT.md](DEPLOYMENT.md). For controls not yet implemented (RBAC,
encryption at rest, audit logging), front AuraDB with infrastructure that
provides them until they land.
