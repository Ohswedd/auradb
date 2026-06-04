# Certificate directory

This directory holds the TLS material mounted into the container at `/certs` by
`docker-compose.secure.yml`. **No certificates or private keys are committed.**

Generate development certificates (a CA, a server certificate, and keys) here:

```bash
auradb cert generate-dev --out-dir ./examples/production/certs
```

That writes `ca.crt`, `ca.key`, `server.crt`, and `server.key`. The server
certificate is valid for `localhost` and `127.0.0.1`. Development certificates
are for local trials only.

For production, place a certificate and key issued by your certificate authority
here as `server.crt` and `server.key` (and `ca.crt` if you enable mutual TLS).
Keep private keys out of version control; the repository `.gitignore` excludes
`*.key` and `*.crt`.
