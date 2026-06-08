# Release guide

This guide describes how a maintainer cuts an AuraDB release. The current release
is `1.0.1` — the **first production patch on the v1.0 single-node production line**.
Single-node mode is the recommended production mode; multi-node static
clustering remains an HA candidate preview, **not** production HA. AWP 1 and storage
format v2 are **frozen for v1**. See [SUPPORT_POLICY.md](SUPPORT_POLICY.md),
[V1_0_1_RELEASE_NOTES.md](V1_0_1_RELEASE_NOTES.md),
[V1_0_RELEASE_NOTES.md](V1_0_RELEASE_NOTES.md),
[HA_RELEASE_CANDIDATE.md](HA_RELEASE_CANDIDATE.md), and the
[v1.0 decision checklist](V1_0_DECISION_CHECKLIST.md).

### Backup and restore release gate

A release must pass the backup/restore gate over a mixed dataset before tagging,
exercised by the `auradb-cli` backup/restore and upgrade-gate tests
([TESTING.md](TESTING.md)):

1. Back up a mixed dataset (indexes, stats, relationships, vectors, full-text,
   document-path) with `auradb dump`.
2. `auradb backup verify --input <file> --json` (validate without importing).
3. Restore into a fresh single-node directory with `auradb restore`.
4. `auradb check --data-dir <restore> --json`.
5. Query, relationship-include, vector, full-text, and document-path smokes.
6. Index and planner-stats validation.
7. Confirm no secrets in the backup-verify output.

### GitHub Actions maintenance (Node 24)

Workflow actions are kept on majors that run on Node 24, ahead of the Node 20
deprecation. v0.9.2 reported a non-blocking Node 20 warning on
`docker/build-push-action@v6` and `docker/setup-buildx-action@v3`; v1.0.0 resolves
it by upgrading the `docker/*` actions to their Node-24 majors:
`docker/setup-buildx-action` v3 → **v4**, `docker/build-push-action` v6 → **v7**,
`docker/login-action` v3 → **v4**, `docker/metadata-action` v5 → **v6**, and
`docker/setup-qemu-action` v3 → **v4** (the Node-24 majors require Actions Runner
v2.327.1 or later). The non-`docker/*` actions were already on Node-24 majors:
`actions/checkout` (v6), `actions/cache` (v5), `actions/setup-python` (v6),
`actions/upload-artifact` / `actions/download-artifact` (v5),
`dtolnay/rust-toolchain` (Node-free), and `softprops/action-gh-release` (v2). With
the upgrade, **no deprecated-Node-20 major remains pinned**, so no
`FORCE_JAVASCRIPT_ACTIONS_TO_NODE24` mitigation is required. The Docker publish
security posture (permissions, attestations, manifest checks) is unchanged. If an
action later lacks a Node-24 replacement, record it here as a known maintenance
item — and, only if necessary, set `env: FORCE_JAVASCRIPT_ACTIONS_TO_NODE24: true`
on the affected job — rather than pinning a deprecated major.

### HA candidate smoke (v0.9.1, manual / post-release)

- [ ] **HA candidate smoke (manual).** Build the image locally and run the
      leader-change smoke end to end (leader kill → new leader → old-leader
      rejoin → catch-up → status), optionally exercising the connector
      leader-change scenario:

      ```bash
      docker build -t auradb:0.9.1 .
      AURADB_IMAGE=auradb:0.9.1 bash scripts/smoke_ha_candidate.sh
      ```

      In v0.9.1 the smoke prints the old and new leader plus the candidate
      addresses, and the `leader_client_addr` hint reported at each leader, so the
      leader's own client address is visible across the change. It distinguishes
      the **expected in-network/host fallback** (a Docker in-network hint such as
      `node2:7171` is not the host-published port, so a client on the host
      re-resolves the leader by its host port) from a real failure — the fallback
      is documented behavior, not an error.

- [ ] **Published-image HA smoke (post-release).** After the tag publishes the
      image, run the same smoke against it:

      ```bash
      AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.9.1 bash scripts/smoke_ha_candidate.sh
      ```

  These are HA *candidate* smokes, not production HA proof, and are wired as
  manual `workflow_dispatch` jobs in `.github/workflows/cluster.yml` so they
  never block a PR.

### Published-image post-release checklist (v1.0.1)

After the tag publishes the multi-arch image, run **both** published-image smokes
as post-release gates (not PR blockers). Each prints the diagnostics needed to
confirm a clean release and to record as HA-candidate evidence (see
[V1_0_DECISION_CHECKLIST.md](V1_0_DECISION_CHECKLIST.md) §5):

- [ ] **Cluster Compose smoke.** `AURADB_IMAGE=ghcr.io/ohswedd/auradb:1.0.1 bash
      scripts/smoke_cluster_compose.sh` — image used and its digest, the node
      ports, the leader, quorum, per-peer states, and teardown.
- [ ] **HA candidate smoke.** `AURADB_IMAGE=ghcr.io/ohswedd/auradb:1.0.1 bash
      scripts/smoke_ha_candidate.sh` — image digest (when available), the server
      version reported by each node, the leader **before** and **after** the kill,
      the leader **client-address source** (advertised / status / fallback /
      probe), the connector version, and explicit pass/fail criteria.
- [ ] Both smokes preserve logs on failure, tear down cleanly on success, and
      honor `KEEP_ARTIFACTS=1` to retain certs, compose project, and logs for
      inspection.
- [ ] Record the results (leader before/after, client-address source, connector
      version, image digest) as evidence; a passing smoke is HA *candidate*
      evidence, **not** production HA proof.

## Connector-first coordinated releases

Some AuraDB releases coordinate with an Aura Connector release (e.g. AuraDB v0.7.1
with Aura Connector v0.4.1). When the connector changed, **release the connector
first** so AuraDB conformance can run against the published client:

1. Release Aura Connector first (tag, publish to PyPI, verify a clean
   `pip install aura-connector==<x.y.z>` in a fresh venv).
2. Re-run AuraDB's connector cluster conformance against the **published**
   connector. Trigger `.github/workflows/cluster.yml` via *Run workflow* with the
   `require_published_connector` input set so a missing/too-old connector fails
   rather than skips. Locally:

   ```bash
   python -m pip install "aura-connector>=0.4.1,<0.5"
   python tests/conformance/python/run_connector_smoke.py --addr <leader-client-addr>
   python tests/conformance/python/run_connector_conformance.py --addr <leader-client-addr>
   python tests/conformance/python/run_connector_cluster.py \
       --leader <leader-client-addr> --follower <follower-client-addr>
   ```

3. Only after that passes, cut the AuraDB release. Never claim a connector version
   is published before it actually is; until then the conformance step skips with
   a clear message on PR/push.

## Pre-release checklist

- [ ] `CHANGELOG.md` has an entry for the new version with today's date.
- [ ] Workspace version in `Cargo.toml` is bumped (and `Cargo.lock` regenerated).
- [ ] Documentation reflects any new or changed behavior.
- [ ] The support policy ([SUPPORT_POLICY.md](SUPPORT_POLICY.md)) and the support
      matrix ([COMPATIBILITY.md](COMPATIBILITY.md)) are current.
- [ ] AWP 1 and storage format v2 freeze statements are present and accurate.
- [ ] The backup/restore release gate passes (see above and
      [PRODUCTION_READINESS.md](PRODUCTION_READINESS.md)).
- [ ] The GitHub release body carries the single-node production statement, the
      multi-node preview disclaimer, the AWP 1 and storage v2 statements, and the
      known limitations (verified by `verify_release_artifacts.sh --tag`).
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
      `auradb bench --json --output benches/baseline/<version>.json` (v1.0.1
      commits `benches/baseline/v1.0.1.json`). Benchmark numbers are
      machine-specific and **warn-only** — never a release gate.

### Multi-node preview validation (v0.5.x, fail-stop recovery in v0.6.0)

- [ ] **Local-image Docker cluster smoke (required path).** Build the image
      locally and run the live three-node Compose cluster end to end via
      `AURADB_IMAGE` (no registry pull, so it never depends on GHCR publish
      timing), or rely on the cluster CI workflow when Docker is unavailable:

      ```bash
      docker build -t auradb:0.7.1 .
      AURADB_IMAGE=auradb:0.7.1 bash scripts/smoke_cluster_compose.sh
      ```

- [ ] **Published-image smoke (post-release verification).** After the release
      tag has published the image to GHCR, verify it with the same smoke:

      ```bash
      AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.7.1 bash scripts/smoke_cluster_compose.sh
      ```

### Published GHCR cluster smoke checklist (v0.7.1)

Run this after the release tag has published the image. The manual
`published-image-smoke` job in `.github/workflows/cluster.yml` performs the same
steps in CI.

- [ ] **Wait for the GHCR publish to complete** (the `Docker` workflow's `publish`
      job on the `v0.7.1` tag).
- [ ] **Inspect the multi-arch manifest** and confirm both platforms are present:

      ```bash
      docker buildx imagetools inspect ghcr.io/ohswedd/auradb:0.7.1
      # expect: linux/amd64 and linux/arm64
      ```

- [ ] **Pull each platform where possible** (arm64 on Apple Silicon, amd64 on
      x86_64):

      ```bash
      docker pull --platform linux/amd64 ghcr.io/ohswedd/auradb:0.7.1
      docker pull --platform linux/arm64 ghcr.io/ohswedd/auradb:0.7.1
      docker run --rm ghcr.io/ohswedd/auradb:0.7.1 auradb version   # prints auradb 0.7.1
      ```

- [ ] **Run the Compose smoke against the published image.** The script prints
      the image, node ports, leader, quorum, peer states, and teardown result:

      ```bash
      AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.7.1 bash scripts/smoke_cluster_compose.sh
      ```

- [ ] **Confirm the release notes state the preview limits** (not production HA;
      single-node remains the recommended production mode) — see
      [V0_6_1_RELEASE_NOTES.md](V0_6_1_RELEASE_NOTES.md).

### Multi-arch Docker publish (v0.7.1)

- [ ] The `Docker` workflow's `publish` job (on the `v0.7.1` tag) builds a
      `linux/amd64,linux/arm64` manifest with Buildx + QEMU and pushes it to
      `ghcr.io/ohswedd/auradb:0.7.1` and `:latest`.
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

### Verifying release artifacts

`scripts/verify_release_artifacts.sh` checks a release for completeness and
integrity. It runs in three modes:

```bash
# Verify a local directory of built artifacts (no network).
scripts/verify_release_artifacts.sh --dir out --version 1.0.1

# Download and verify the published assets for a tag (requires the gh CLI), and
# confirm the release body carries the required v1.0 statements.
scripts/verify_release_artifacts.sh --tag v1.0.1

# Network-free self-test of the verifier itself (good dir passes; missing
# archive, bad checksum, and wrong-version name each fail).
scripts/verify_release_artifacts.sh --self-test
```

It verifies that all five expected platform archives are present, that `SHA256SUMS`
lists **and** matches every archive (no stray, unlisted asset ships), that archive
names carry the version, and that a host-matching binary prints the expected
`auradb 1.0.1`. In `--tag` mode it additionally confirms the GitHub release body
carries the **single-node production support statement**, the **multi-node preview
disclaimer**, the **AWP 1 statement**, the **storage format v2 statement**, and
**known limitations**. The CI `release.yml` runs `--dir` verification after the
collect step; the `--tag` body-statement check and the published-image cluster and
HA candidate smokes remain post-release steps.

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
