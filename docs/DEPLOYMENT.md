# Deployment

This guide covers running AuraDB beyond local development: with authentication
and TLS enabled, behind Docker, with a production configuration template and a
secret that never lands in version control.

For local development, see the quickstart in the [README](../README.md) and
[`examples/auradb.local.toml`](../examples/auradb.local.toml).

## Security defaults

AuraDB fails closed:

- A non-loopback bind (for example `0.0.0.0`) with authentication disabled is
  rejected at startup unless `allow_insecure_bind = true` (or
  `--allow-insecure-bind`) is set. That override is for development only.
- `auth.enabled = true` without a valid `token_hash` fails startup.
- `tls.enabled = true` without a certificate and key fails startup. AuraDB never
  serves plaintext when TLS is enabled.

The recommended deployment path is the secure Docker Compose example below, which
enables both authentication and TLS.

## Secure Docker Compose

[`docker-compose.secure.yml`](../docker-compose.secure.yml) runs AuraDB with
authentication and TLS enabled, as a non-root user, with a read-only root
filesystem, dropped Linux capabilities, a mounted config, a data volume, and a
mounted certificate directory. The token hash is supplied at runtime through the
`AURADB_AUTH_TOKEN_HASH` environment variable, so no secret is committed.

```bash
# 1. Generate certificates (development certs shown; use real certs in production).
auradb cert generate-dev --out-dir ./examples/production/certs

# 2. Generate a token hash and export it. Never commit the plaintext token.
export AURADB_AUTH_TOKEN_HASH="$(auradb auth hash-token --token 'choose-a-strong-token')"

# 3. Validate the config template (structure only; certs live on the host).
auradb config validate --config ./examples/production/auradb.toml --no-file-checks

# 4. Start it.
docker compose -f docker-compose.secure.yml up
```

At start, the container copies the committed (secret-free) config, appends the
token hash from the environment into the `[auth]` section of a writable copy, and
runs the server against that copy. The plaintext token is never stored.

To validate the compose file without starting anything:

```bash
docker compose -f docker-compose.secure.yml config
```

### Connecting

```bash
auradb status --addr 127.0.0.1:7171 \
  --tls-ca ./examples/production/certs/ca.crt \
  --server-name localhost \
  --token 'choose-a-strong-token'
```

The health probe is allowed unauthenticated but still requires TLS, so the
container healthcheck trusts the CA bundle.

### Validation status

This secure Compose example was validated at runtime for the v0.2.1 release with
development certificates (`auradb cert generate-dev`) and a generated token hash
(`auradb auth hash-token`). The container reached a healthy state over TLS with
authentication, a plaintext client was rejected, the Aura Connector smoke passed
against it over TLS plus auth, and the token, its hash, and the private key did
not appear in the container logs.

Development certificates are for local trials only. For a production deployment,
use certificates issued by your certificate authority and inject the token hash
from a managed secret store (for example Docker secrets, or your orchestrator's
secret mechanism) rather than from a shell variable.

## Production configuration template

[`examples/auradb.secure.toml`](../examples/auradb.secure.toml) is a standalone
template for a non-container deployment, and
[`examples/production/auradb.toml`](../examples/production/auradb.toml) is the
container variant used by the secure Compose file. Both enable authentication
and TLS, set conservative payload and cursor limits, and emit structured JSON
logs. Neither contains a plaintext secret.

Validate a template whose certificates live on the target host with
`--no-file-checks`, which checks structure (auth hash shape, enabled-without-cert
paths, insecure public bind) without requiring the certificate files to exist on
the machine running the check:

```bash
auradb config validate --config examples/auradb.secure.toml --no-file-checks
```

## Certificates

Generate development certificates (a CA, a server certificate, and keys) with:

```bash
auradb cert generate-dev --out-dir ./certs
```

The server certificate is valid for `localhost` and `127.0.0.1` and is for local
trials only. For production, use certificates issued by your certificate
authority. Keep private keys out of version control; the repository `.gitignore`
excludes `*.key`, `*.crt`, `*.pem`, and `.env`.

For mutual TLS, set `tls.require_client_cert = true` and `tls.client_ca_path` to
the CA bundle that signs your client certificates.

## Token rotation

Rotate the static token without hand-editing the config:

```bash
auradb auth rotate-token --config /etc/auradb/auradb.toml --token 'new-strong-token' --backup
```

This re-hashes the new token with Argon2id, writes the configuration atomically,
preserves unrelated fields, optionally backs up the previous config to
`<config>.bak`, and validates the result. A running server keeps the token it
loaded at startup; restart (or redeploy) the server to enforce the new token.
AuraDB does not hot-reload the token. See [SECURITY.md](SECURITY.md).

## Operational checks

```bash
# Local data-directory health as JSON.
auradb doctor --data-dir /var/lib/auradb --json

# A running server's status as JSON (over TLS, with a token).
auradb status --addr 127.0.0.1:7171 --tls-ca ./certs/ca.crt --token 'your-token' --json
```

Both redact secrets. See [OBSERVABILITY.md](OBSERVABILITY.md).
