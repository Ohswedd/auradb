# Contributing to AuraDB

Thanks for your interest in AuraDB. This guide covers the workflow and the
quality bar every change must meet. By participating you agree to the
[Code of Conduct](CODE_OF_CONDUCT.md).

## Toolchain setup

AuraDB builds with a stable Rust toolchain (1.85 or newer). Install Rust with
[rustup](https://rustup.rs) and add the components used by CI:

```bash
rustup component add rustfmt clippy
```

## Development workflow

1. Open or pick an issue, or check the [roadmap](docs/ROADMAP.md) for direction.
2. Implement the change in the appropriate crate, keeping crate boundaries and
   the dependency direction in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
   intact.
3. Add tests alongside the change (see [`docs/TESTING.md`](docs/TESTING.md)).
4. Run the full validation suite below.
5. Update the relevant `docs/` file and `CHANGELOG.md`.

## Validation

Every change must pass:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-features
```

## Conformance tests

The Rust conformance suite runs over the wire protocol:

```bash
cargo test -p auradb-conformance
```

When the official `aura-connector` Python package is available, the Python
harness can be run against a live server:

```bash
python -m pip install aura-connector
python tests/conformance/python/run_conformance.py --addr 127.0.0.1:7171
```

## Benchmarks

Criterion benchmarks live across the workspace. Run them when a change can affect
performance:

```bash
cargo bench --workspace
```

Keep benchmarks measuring real code. Do not commit fabricated numbers.

## Engineering rules

- **No misleading behavior.** Do not present unimplemented features as working.
  Unsupported operations return a structured `Error::Unsupported`.
- **Fail closed.** Never silently ignore a failed write, flush, checksum, or
  recovery error.
- **Safe Rust.** `unsafe` is forbidden by `#![forbid(unsafe_code)]` in every
  crate. If you believe an exception is warranted, raise it for review with
  invariants and benchmarks.
- **Honest claims.** Documentation describes what is implemented; experimental or
  server-only features are labeled as such.
- **Typed errors in libraries; `anyhow` only at the CLI boundary.**

## Commit and pull request conventions

- Keep commits focused and message bodies explanatory.
- Pull requests should describe the change, the testing performed, and any new
  limitations.
- Do not add co-author trailers unless a maintainer explicitly requests them.
