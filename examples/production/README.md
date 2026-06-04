# Production deployment example

A secure AuraDB deployment with authentication and TLS enabled, driven by
[`docker-compose.secure.yml`](../../docker-compose.secure.yml) at the repository
root.

Files:

- `auradb.toml` - the runtime configuration (auth and TLS on). It carries **no
  secret**; the token hash is injected at container start.
- `certs/` - where the TLS certificate and key live. Nothing here is committed;
  see [`certs/README.md`](certs/README.md).

## Quick start

```bash
# 1. Generate development certificates (use real certs in production).
auradb cert generate-dev --out-dir ./examples/production/certs

# 2. Generate a token hash and export it. Never commit the plaintext token.
export AURADB_AUTH_TOKEN_HASH="$(auradb auth hash-token --token 'choose-a-strong-token')"

# 3. Validate the config template (structure only; certs live on the host).
auradb config validate --config ./examples/production/auradb.toml --no-file-checks

# 4. Start the server.
docker compose -f docker-compose.secure.yml up
```

Connect a client over TLS, trusting the generated CA, and authenticating with
the token you chose:

```bash
auradb status --addr 127.0.0.1:7171 \
  --tls-ca ./examples/production/certs/ca.crt \
  --server-name localhost \
  --token 'choose-a-strong-token'
```

## How the secret is handled

The committed `auradb.toml` has `[auth] enabled = true` but no `token_hash`. At
start, the container appends the hash from `AURADB_AUTH_TOKEN_HASH` to a writable
copy of the config and runs the server against that copy. The plaintext token is
never stored, and no secret is written to version control.

If you prefer Docker secrets, mount the hash at `/run/secrets/auradb_token_hash`
and read it in the entrypoint instead of the environment variable.

## Rotating the token

```bash
# On the host, against the template, then redeploy with the new hash:
auradb auth rotate-token --config ./examples/production/auradb.toml --token 'new-strong-token' --backup
```

A running server keeps the token it loaded at startup; restart (redeploy) the
container to enforce the new token. See [docs/SECURITY.md](../../docs/SECURITY.md).
