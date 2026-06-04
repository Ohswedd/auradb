# Release guide

This guide describes how a maintainer cuts an AuraDB release.

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

CI must be green on the target branch (build, lint, test, and security
workflows).

## Docker

Build and smoke-test the image:

```bash
docker build -t auradb:release .
docker run --rm auradb:release auradb version
```

## Tag and GitHub release

```bash
git tag v0.1.0
git push origin v0.1.0
```

Then create a GitHub release from the tag. Use the matching `CHANGELOG.md`
section as the release notes.

## Publishing (optional)

- **crates.io.** Publish crates only if intended, in dependency order from
  `auradb-core` upward. Verify each crate's metadata first with
  `cargo publish --dry-run`.
- **Docker image.** Publish the image to a registry only if intended, tagging it
  with the release version and `latest`.

## Post-release

- [ ] Open a new "Unreleased" section in `CHANGELOG.md` for ongoing work.
- [ ] Confirm the published artifacts install and run from a clean environment.
