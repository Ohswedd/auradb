# Release guide

This guide describes how a maintainer cuts an AuraDB release. The current release
is `0.2.0`.

## Pre-release checklist

- [ ] `CHANGELOG.md` has an entry for the new version with today's date.
- [ ] Workspace version in `Cargo.toml` is bumped.
- [ ] Documentation reflects any new or changed behavior.
- [ ] All limitations are stated honestly; nothing unimplemented is claimed.

## Validation

Run the full suite locally before tagging:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-features --release
```

CI must be green on the target branch: `ci.yml` (fmt, clippy, test, build),
`security.yml` (cargo audit and deny), `conformance.yml` (Python AWP conformance
for auth disabled, auth enabled, and TLS), and `docker.yml` (build and smoke).

## Docker

`docker.yml` builds and smoke-tests the image on every push and, on a version
tag, publishes it to the GitHub Container Registry at `ghcr.io/ohswedd/auradb`.
The image is a multi-stage build (a `rust:1.85-slim` build stage that installs
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
git tag v0.2.0
git push origin v0.2.0
```

Pushing the tag triggers `release.yml` (binaries and `SHA256SUMS`) and the
publish step of `docker.yml` (GHCR image). Create the GitHub release from the
tag and use the matching `CHANGELOG.md` section, or
[V0_2_RELEASE_NOTES.md](V0_2_RELEASE_NOTES.md), as the release notes.

## Publishing (optional)

- **crates.io.** Publish crates only if intended, in dependency order from
  `auradb-core` upward. Verify each crate's metadata first with
  `cargo publish --dry-run`.

## Post-release

- [ ] Open a new "Unreleased" section in `CHANGELOG.md` for ongoing work.
- [ ] Confirm the published artifacts and the GHCR image install and run from a
  clean environment.
