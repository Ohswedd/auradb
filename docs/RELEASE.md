# Release guide

This guide describes how a maintainer cuts an AuraDB release. The current release
is `0.6.1`.

## Pre-release checklist

- [ ] `CHANGELOG.md` has an entry for the new version with today's date.
- [ ] Workspace version in `Cargo.toml` is bumped.
- [ ] Documentation reflects any new or changed behavior.
- [ ] All limitations are stated honestly; nothing unimplemented is claimed.
- [ ] The backup/restore, v0.1.0 and v0.2.x upgrade, MVCC/snapshot-isolation,
      planner, and chaos restart tests pass.
- [ ] The cluster metadata, Raft log, Raft consensus (deterministic in-process),
      replicated apply, snapshot, and single-node cluster tests pass.
- [ ] The multi-node preview tests pass: peer transport (frame codec and
      `PeerHello` handshake), cross-process replication, leader/follower client
      behavior, and the live cluster CLI commands.
- [ ] Cluster mode limitations are stated honestly: single-node remains the
      recommended production path; the multi-node server preview is experimental,
      off by default, and gated by two opt-ins.
- [ ] The benchmark baseline under `benches/baseline/` is refreshed on the
      release machine with
      `auradb bench --json --output benches/baseline/<version>.json`.

### Multi-node preview validation (v0.5.x, fail-stop recovery in v0.6.0)

- [ ] **Local-image Docker cluster smoke (required path).** Build the image
      locally and run the live three-node Compose cluster end to end via
      `AURADB_IMAGE` (no registry pull, so it never depends on GHCR publish
      timing), or rely on the cluster CI workflow when Docker is unavailable:

      ```bash
      docker build -t auradb:0.6.1 .
      AURADB_IMAGE=auradb:0.6.1 bash scripts/smoke_cluster_compose.sh
      ```

- [ ] **Published-image smoke (post-release verification).** After the release
      tag has published the image to GHCR, verify it with the same smoke:

      ```bash
      AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.6.1 bash scripts/smoke_cluster_compose.sh
      ```

### Published GHCR cluster smoke checklist (v0.6.1)

Run this after the release tag has published the image. The manual
`published-image-smoke` job in `.github/workflows/cluster.yml` performs the same
steps in CI.

- [ ] **Wait for the GHCR publish to complete** (the `Docker` workflow's `publish`
      job on the `v0.6.1` tag).
- [ ] **Inspect the multi-arch manifest** and confirm both platforms are present:

      ```bash
      docker buildx imagetools inspect ghcr.io/ohswedd/auradb:0.6.1
      # expect: linux/amd64 and linux/arm64
      ```

- [ ] **Pull each platform where possible** (arm64 on Apple Silicon, amd64 on
      x86_64):

      ```bash
      docker pull --platform linux/amd64 ghcr.io/ohswedd/auradb:0.6.1
      docker pull --platform linux/arm64 ghcr.io/ohswedd/auradb:0.6.1
      docker run --rm ghcr.io/ohswedd/auradb:0.6.1 auradb version   # prints auradb 0.6.1
      ```

- [ ] **Run the Compose smoke against the published image.** The script prints
      the image, node ports, leader, quorum, peer states, and teardown result:

      ```bash
      AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.6.1 bash scripts/smoke_cluster_compose.sh
      ```

- [ ] **Confirm the release notes state the preview limits** (not production HA;
      single-node remains the recommended production mode) — see
      [V0_6_1_RELEASE_NOTES.md](V0_6_1_RELEASE_NOTES.md).

### Multi-arch Docker publish (v0.6.1)

- [ ] The `Docker` workflow's `publish` job (on the `v0.6.1` tag) builds a
      `linux/amd64,linux/arm64` manifest with Buildx + QEMU and pushes it to
      `ghcr.io/ohswedd/auradb:0.6.1` and `:latest`.
- [ ] PR/branch builds build `linux/amd64` through buildx **without** publishing.
- [ ] Local validation built `linux/amd64` only
      (`docker buildx build --platform linux/amd64 --load`); the arm64 image is
      built by CI under QEMU and verified via `docker buildx imagetools inspect`.
- [ ] The existing binary release artifacts (`release.yml`) are unaffected.

- [ ] **Three-node loopback smoke.** Start the three local nodes and confirm an
      election, leader-routed writes, `not_leader` from a follower, and follower
      catch-up after restart:

      ```bash
      auradb server --config examples/cluster/node1.toml
      auradb server --config examples/cluster/node2.toml
      auradb server --config examples/cluster/node3.toml
      auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30
      auradb cluster leader      --addr 127.0.0.1:7171 --json
      auradb status              --addr 127.0.0.1:7171 --json   # per-peer state
      ```

- [ ] **Cluster CLI.** `auradb cluster leader|wait-leader|wait-ready` against a
      running node return correctly in text and `--json`.
- [ ] **Docker Compose config check.** The Docker-network preview (peer TLS +
      token) validates structurally:

      ```bash
      docker compose -f docker-compose.cluster.yml config
      ```

## Validation

Run the full suite locally before tagging:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-features --release
```

Also validate the official client and the secure deployment locally before
tagging, using a virtual environment under an ignored path:

```bash
python3 -m venv .local/connector-venv && . .local/connector-venv/bin/activate
python -m pip install "aura-connector>=0.3,<0.4"
# Start a server (plaintext, then auth, then TLS plus auth) and run:
python tests/conformance/python/run_connector_smoke.py --addr 127.0.0.1:7171
# ...repeating with --auth-token and --tls-ca as appropriate.

# Secure Compose runtime, with development certs and a generated token hash:
auradb cert generate-dev --out-dir ./examples/production/certs
export AURADB_AUTH_TOKEN_HASH="$(auradb auth hash-token --token 'a-strong-token')"
docker compose -f docker-compose.secure.yml up -d   # expect a healthy container
docker compose -f docker-compose.secure.yml down -v
```

For the v0.3.0 release these were validated locally with `aura-connector` 0.3.0:
the connector smoke passed in plaintext, auth, and TLS-plus-auth modes, the full
connector conformance passed over TLS plus auth, and the secure Compose container
reached a healthy state over TLS with authentication with no secret in its logs.
Aura Connector 0.3.x remains compatible with AuraDB 0.3.0; no connector release is
required.

CI must be green on the target branch: `ci.yml` (fmt, clippy, test including the
backup/restore, upgrade, and chaos tests, build, and benchmark compilation),
`security.yml` (cargo audit and deny), `conformance.yml` (Python AWP conformance
for auth disabled, auth enabled, and TLS, plus the Aura Connector smoke and
conformance suites), and `docker.yml` (build and smoke).

## Docker

`docker.yml` builds and smoke-tests the image on every push and, on a version
tag, publishes it to the GitHub Container Registry at `ghcr.io/ohswedd/auradb`.
The image is a multi-stage build (a `rust:1.90-slim-bookworm` build stage that installs
`build-essential` for the TLS stack, and a `debian:bookworm-slim` runtime),
runs as a non-root user (uid 10001), exposes `7171`, defaults its command to
`auradb server`, stores data in the `/data` volume, and ships a `HEALTHCHECK`
that calls `auradb status`. On a version tag the image is tagged with `latest`,
the bare version, the `v`-prefixed tag, and `sha-<short>`.

Build and smoke-test locally before tagging:

```bash
docker build -t auradb:release .
docker run --rm auradb:release auradb version
```

## Binary release artifacts

The `release.yml` workflow triggers on `v*` tags and builds binaries for:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

Each archive (`auradb-vX.Y.Z-<target>.tar.gz`, or `.zip` on Windows) contains the
`auradb` binary, the README, the LICENSE, and an example config. A `SHA256SUMS`
file is generated, and all archives plus the checksum file are attached to the
GitHub release.

## Tag and GitHub release

```bash
git tag v0.3.0
git push origin v0.3.0
```

Pushing the tag triggers `release.yml` (binaries and `SHA256SUMS`) and the
publish step of `docker.yml` (GHCR image). Create the GitHub release from the
tag and use the matching `CHANGELOG.md` section as the release notes.

## Publishing (optional)

- **crates.io.** Publish crates only if intended, in dependency order from
  `auradb-core` upward. Verify each crate's metadata first with
  `cargo publish --dry-run`.

## Post-release

- [ ] Open a new "Unreleased" section in `CHANGELOG.md` for ongoing work.
- [ ] Confirm the published artifacts and the GHCR image install and run from a
  clean environment.
