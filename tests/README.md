# Tests layout

Cargo runs integration tests from each crate's own `tests/` directory, so the
substantive test suites live there:

| Category | Location |
|---|---|
| Engine integration (CRUD, vectors, relationships, transactions, recovery) | `crates/auradb/tests/engine.rs` |
| Server integration (concurrent clients, malformed frames, limits) | `crates/auradb-server/tests/integration.rs` |
| Protocol property/fuzz | `crates/auradb-protocol/tests/fuzz.rs` |
| Conformance over TCP + restart | `crates/auradb-conformance/tests/conformance.rs` |
| Storage / recovery (torn tail, corruption, restart) | `crates/auradb-storage/src/lib.rs` + `format.rs` unit tests |

Run everything with:

```bash
cargo test --workspace --all-features
```

## Python conformance harness

A runnable cross-language harness lives in
[`conformance/python/`](conformance/python/). See its README.
