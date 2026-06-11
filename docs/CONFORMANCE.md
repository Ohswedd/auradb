# Aura Connector Conformance

AuraDB is tested against client-side scenarios that mirror Aura Connector usage,
over the real wire protocol.

## Two harnesses

1. **Rust** (`crates/auradb-conformance`) - a `Client` implementing the client
   side of AWP and a scenario suite (`run_all`). The integration test
   `crates/auradb-conformance/tests/conformance.rs` starts a real server on an
   ephemeral port and asserts every scenario passes; it also verifies data
   survives a server restart.

2. **Python** (`tests/conformance/python/run_conformance.py`) - a self-contained
   AWP client (standard library only) that runs the same scenarios against a
   running server. It demonstrates cross-language wire compatibility and accepts
   `--auth-token`, `--tls-ca`, and `--tls-server-name` to exercise authenticated
   and TLS-terminated servers over the wire.

## Scenarios

ping, health, schema create, insert, find, filter, document field, document-path
index (with an EXPLAIN check), full-text search (with an EXPLAIN check),
relationship include, vector nearest, explain, count, exists, migration
estimate, update/upsert/delete, transaction commit/rollback, and
transaction-scoped reads (a staged write is visible to the transaction's own
read but not to a non-transactional read until commit). The Rust test also
forces cursor streaming via a small page size.

### MVCC and planner scenarios (0.3.0)

- **`snapshot_isolation_later_commit_invisible`** - a transaction that pins its
  snapshot at `begin` does not observe a write another transaction commits
  afterward.
- **`write_conflict_rejected`** - committing a transaction whose write set was
  modified concurrently is rejected with a conflict (first-committer-wins).
- **`explain_analyze_shape`** - `EXPLAIN ANALYZE` (requested via the raw Query IR
  `"analyze": true` flag) returns the plan plus execution metrics
  (scanned/matched/returned rows, execution and planning time, snapshot ts).
- **`planner_uses_index`** - the cost-based planner selects an index access path
  for a selective equality rather than a full scan.

These run as part of the conformance suite alongside the scenarios above.

### Search and ranking scenarios (1.1.0)

- **`text_search_bm25`** - a BM25 ranked full-text query returns documents ordered
  by relevance (the dense, short document ranks first) with a 1-based `rank`.
- **`hybrid_search`** - a hybrid text+vector query returns rows with a fused score.
- **`search_explain_analyze`** - `EXPLAIN ANALYZE` of a ranked text query returns the
  ranked-text plan summary plus execution metrics.

These run as part of `run_all` alongside the scenarios above (exercising the additive
`text_search` and `hybrid` Query IR clauses over AWP 1), and survive the
`data_survives_server_restart` integration test.

### Query-ergonomics scenarios (1.2.0)

- **`aggregate_count_min_max`** - the `aggregate` request returns `count`/`min`/`max`
  metrics matching the live collection.
- **`terms_facet_index_backed`** - a terms facet over an equality-indexed field is
  served from the index (`used_index`) with deterministic count-desc / value-asc buckets.
- **`search_facet_bm25`** - a facet scoped to a BM25 candidate set (`search_scoped`).
- **`vector_ann_preview`** - an opt-in approximate (HNSW) vector query returns neighbours
  and `EXPLAIN` reports `approximate`. Exact vector search remains the correctness baseline.
- **`ranked_pagination_search_page`** - paging a ranked search by `search_page` cursor
  token over the wire reconstructs the full ranked order with no duplicates.

These exercise the additive `aggregate` / `search_page` read requests and the
`vector_ann` option over AWP 1.

### Live v1.2 connector conformance (1.2.1)

AuraDB v1.2.1 adds over-the-wire connector-driven harnesses that exercise the v1.2
query features against a **running server** through the published Aura Connector
v0.6.1 API (never the in-memory backend). Each prints a `PASS`/`FAIL` line per check
and exits non-zero on failure:

- **`run_connector_facets.py`** — terms facets (basic, limit, deterministic
  count-desc / value-asc tie-break), `count`/`min`/`max` aggregations (all and
  filtered), BM25 search-scoped facets, and a clear capability error on a backend
  that cannot serve them (≥ 8 checks).
- **`run_connector_pagination.py`** — ranked pagination by stable cursor token:
  duplicate-free pages across BM25, hybrid, and exact-vector ranking, cursor-token
  presence, structured invalid-cursor rejection, and transaction-snapshot stability
  guidance (≥ 7 checks).
- **`run_connector_timeouts.py`** — per-query `timeout_ms` acceptance, a real 1ms
  full-scan that returns a structured `query_timeout` error, that the connection
  survives a timeout, and the cooperative nature documented honestly (≥ 5 checks).

The non-cluster scripts run in the `Conformance` workflow's `connector` job against a
live server using the paired connector. They require Aura Connector **v0.6.1**, which
forwards the per-query `timeout_ms` to the wire so `.timeout(ms)` is enforced by
AuraDB (v0.6.0 silently dropped it for the AuraDB backend; the timeout harness fails
on v0.6.0).

**Cluster variants are operator-run, not CI-gated.**
`run_connector_facets_cluster.py`, `run_connector_pagination_cluster.py`, and
`run_connector_timeouts_cluster.py` drive the same features against a `--leader` and
`--follower`, assert leader-only writes (`AuraNotLeaderError`), redirect-preserves-query,
and feature correctness after a leader change (with `--candidate-addrs`), and record
follower reads as **eventually consistent, never linearizable**. Their leader-change
step requires stopping a node and the timeout variant seeds a large replicated dataset,
so they are run by an operator against a local cluster rather than gated as required CI.
Launch a loopback cluster (`bash scripts/smoke_cluster_loopback.sh` starts three nodes
from `examples/cluster/node{1,2,3}.toml`) and run, for example:

```bash
python tests/conformance/python/run_connector_facets_cluster.py \
  --leader 127.0.0.1:7171 --follower 127.0.0.1:7181 \
  --candidate-addrs 127.0.0.1:7171,127.0.0.1:7181,127.0.0.1:7191
```

Multi-node remains an HA candidate preview, **not production HA**.

The Python connector search harness `tests/conformance/python/run_connector_search.py`
drives BM25, exact vector, and hybrid search and capability negotiation against a live
server through Aura Connector v0.5.0. For cluster mode,
`tests/conformance/python/run_connector_search_cluster.py` adds: BM25 and hybrid on the
leader, leader-only search-index writes, redirect-preserves-search-query, transaction
no-auto-redirect, search after a leader change, and a non-failing observation of the
follower read behavior (followers serve eventually-consistent, non-linearizable reads — send
search to the leader for correctness). Multi-node remains an HA candidate preview, not
production HA. At the engine level,
`crates/auradb-replication/tests/multi_node.rs::cluster_search_bm25_and_hybrid_after_replication`
confirms a follower rebuilds its BM25 and vector indexes from the replicated log.

### Live v1.3 connector conformance (1.3.0)

AuraDB v1.3.0 adds over-the-wire connector-driven harnesses for the v1.3 query
features, exercised against a **running server** through the published Aura
Connector v0.7.x API (never the in-memory reference backend). Each prints a
`PASS`/`FAIL` line per check and exits non-zero on failure:

- **`run_connector_group_by.py`** — single-field GROUP BY: deterministic
  count-desc / key-asc group ordering, per-group `min`/`max`, `group_limit`
  truncation with an honest `group_count_total`, filter scoping, BM25
  search-candidate scoping, and a clear capability error on an unsupported
  backend.
- **`run_connector_ann_preview.py`** — the opt-in approximate (HNSW) vector
  preview via `search_vector(..., approximate=HnswOptions(...))`: results
  returned, recall-vs-exact above a dataset-specific floor, the default
  `fallback="exact"` baseline, and the honest `fallback="error"` policy (serve
  the preview or return a structured error — never a silent wrong answer).
- **`run_connector_cursor_resume.py`** — public ranked-cursor resume:
  `builder.page(page_size=...)` then `client.resume_search(builder, cursor)` from
  an externally-held opaque token, duplicate-free pages, opaque-token check, and
  structured invalid-token rejection.
- **`run_connector_query_profile.py`** — the best-effort EXPLAIN ANALYZE query
  profile via `.profile()`: requesting a profile never breaks the query, the
  `QueryProfile` is well typed when present (advisory fields may be absent), and
  the client-side query IR is still exposed via `.explain()`.

These require Aura Connector **v0.7.0**.

### v1.5.0 analyzer & snippet harnesses

AuraDB v1.5.0 adds two connector-driven harnesses for the live, over-the-wire
query-time analyzer and snippet surface. They require **Aura Connector v0.9.0** and a
running v1.5.0 server, use dedicated collections so they never collide with other
suites, print a `PASS`/`FAIL` line per check, and exit non-zero on failure. They are
**release gates**: each fails (does not skip) if the server does not advertise the
capability it needs.

- **`run_connector_analyzers.py`** — live query-time analyzer selection via
  `search_text(analyzer=…)` / `.analyzer(…)`: the `query_analyzers` capability is
  present; `default` matches an existing (no-analyzer) search; `simple` case
  behavior; `ascii_fold` recovers an unaccented query against an accented document
  (where `simple` does not); `keyword` whole-field exact match; `english_basic`
  plural recall and the `english_basic_lens_regression` (a bare-`s` singular like
  `lens` is not truncated to `len`); the `keyword` analyzer in **hybrid** search
  (`hybrid_keyword_analyzer_success`, the vector component still contributing, and
  the profile/explain reporting `analyzer="keyword"`); the chained `.analyzer()` live
  path; a profiled analyzer query; and the structured client-side unknown-analyzer
  error.
- **`run_connector_snippets.py`** — live opt-in snippets via
  `QueryBuilder.snippets(...)` + `aura.search_snippets(row)`: the `search_snippets`
  capability is present; opt-in only (no request → no snippets); a basic highlighted
  fragment whose range slices the matched text; the field allowlist and the
  no-hidden-fields guarantee (a `secret` field is never returned); fragment/char
  caps; Unicode safety; missing-field safety; and the typed result models.

Run them alongside the other suites:

```bash
python tests/conformance/python/run_connector_analyzers.py --addr <leader-client-addr>
python tests/conformance/python/run_connector_snippets.py  --addr <leader-client-addr>
```

These are not CI-required (they need a running server), but they are local release
gates for v1.5.0. The cluster search-analytics drill
`scripts/smoke_cluster_search_analytics.sh` runs the search/facets/pagination/
group-by/cursor-resume suite against a three-node Compose cluster leader, then
performs a bounded leader-change drill (stop the leader, wait for a new one,
re-run the checks) and confirms quorum after the old node rejoins. It is an
operator-run **HA candidate preview** drill — not production HA proof and not an
automatic-failover SLA.

### Cluster scenarios (0.4.0)

These scenarios exercise single-node cluster mode end to end. They confirm the
cluster path works without changing the non-cluster guarantees:

- **`single_node_cluster_connect`** - a server started with `[cluster] enabled =
  true` and no peers accepts connections and serves requests normally.
- **`cluster_status_and_capability`** - the health report includes the additive
  `cluster` section (node id, cluster id, role `leader`, term, commit/applied
  indices, `single_node = true`, replication lag) and the wire protocol version is
  unchanged.
- **`leader_accepts_writes`** - the single node is the leader and accepts writes;
  there is no `not_leader` rejection in single-node mode.
- **`raft_backed_write_survives_restart`** - a write committed through the Raft log
  is present after the server restarts (committed-but-unapplied entries replay).
- **`snapshot_create_restore`** - a snapshot captures schemas and current records
  and restores them into a fresh engine with identical visible state.
- **`non_cluster_mode_unchanged`** - with cluster mode disabled (the default),
  every scenario above and every prior scenario still passes, confirming the
  default path is unchanged.

These run as part of the conformance suite. See [CLUSTERING.md](CLUSTERING.md) and
[REPLICATION.md](REPLICATION.md).

### Three-node preview scenarios (0.5.0)

> **AuraDB v0.5.0 introduces a controlled, experimental multi-node server
> preview. Single-node mode remains the recommended production mode.**

The v0.5.0 multi-node preview is exercised across real server processes over real
TCP sockets (the loopback three-node configuration). The scenarios confirm the
cross-process cluster behaves as described:

- **Detect leader** — after the cluster elects, a leader is reported (via
  `auradb cluster leader` / the `cluster` status section).
- **Write to leader** — a write sent to the leader's client address is accepted,
  replicated, and committed on a majority.
- **Follower returns `not_leader`** — a write sent to a follower returns the
  structured `not_leader` error with a leader hint.
- **Follower health / status** — a follower stays healthy and reports per-peer
  cluster state (`preview_multi_node`, `quorum_available`, and the `peers` array)
  in `auradb status --json`.
- **Stop + restart follower catch-up** — a stopped follower, after restart,
  replays its durable log and is brought current by the leader.

The Aura Connector validates against the **leader's** client address; a write
routed to a follower surfaces `not_leader`, which a 0.3.x connector handles
additively. For v0.5.0 the published `aura-connector` 0.3.0 smoke suite was run
against the elected leader of a loopback three-node cluster (12/12 checks passed);
the auth/TLS connector matrix and full conformance suite run in `conformance.yml`.
See [CLUSTERING.md](CLUSTERING.md) and [TESTING.md](TESTING.md).

### v0.5.1 hardening

v0.5.1 keeps the above scenarios and adds coverage exercised by the cluster CI
workflow: **leader restart and re-election** (a stopped leader's term is taken
over by the surviving majority; the old leader rejoins as a follower and catches
up), **follower catch-up across 1,000+ entries**, **`not_leader` ergonomics**
(the leader hint carries the leader's client address and the wire error is marked
`retryable`, and the connection stays usable), and **peer TLS validation**
(wrong CA / wrong SAN rejected, rotated certificate accepted). The published
`aura-connector` smoke against the elected leader continues to run in CI; local
runs require PyPI access and are documented rather than faked when offline.

### v0.6.0 fail-stop recovery and snapshot install

v0.6.0 keeps every scenario above and adds **peer snapshot install** coverage: a
follower behind the leader's compacted prefix is restored by a bounded
single-message snapshot install and then resumes AppendEntries, and oversized,
wrong-cluster, bad-digest, and future-format snapshots are rejected without
touching follower state (`crates/auradb-replication/tests/multi_node.rs`).

The published **Aura Connector 0.3.0** was installed from PyPI and run locally
against a v0.6.0 server (the `auradb version` reports `0.6.0`): the AWP protocol
conformance passed **18/18**, the connector smoke **12/12**, and the full
connector conformance **15/15** — no connector changes are required and AWP stays
at v1. The wire additions in v0.6.0 (additive fail-stop diagnostics fields on the
health report's `cluster` section) are ignored by the 0.3.x connector.

### v0.6.1 snapshot install and published-cluster smoke hardening

v0.6.1 keeps every scenario above and adds larger and concurrent-write
snapshot-install coverage (data, index, planner-stats, and MVCC-timestamp
convergence; no duplicate apply under concurrent leader writes) and
snapshot-needed / follower-lag diagnostics with a live `auradb cluster doctor
--addr` (`crates/auradb-replication/tests/multi_node.rs`,
`crates/auradb-cli/tests/cluster_diagnostics.rs`). The connector leader-hint UX
review was **docs-only** (Option A): the `not_leader` leader-hint message and the
no-infinite-retry contract are pinned by
`crates/auradb-server/tests/not_leader.rs`
(`connector_not_leader_message_includes_leader_hint`, `connector_no_infinite_retry`).

For v0.6.1, local validation used the stdlib AWP harness
(`tests/conformance/python/run_conformance.py`, 18/18 against a v0.6.1 server
whose `auradb version` reports `0.6.1`) and the Rust conformance crate
(`auradb-conformance`). Published **Aura Connector 0.3.0** conformance is covered
by CI (`conformance.yml`) and must pass before release — no connector changes are
required and AWP stays at v1. The additive v0.6.1 snapshot/lag diagnostics fields
on the health report's `cluster` section and per-peer status are ignored by the
0.3.x connector.

### v0.6.2 repeated chaos and larger-state recovery hardening

v0.6.2 keeps every scenario above and adds repeated leader restart / re-election,
larger multi-model data-set recovery, multi-model snapshot install, peer
reconnect storms, and network-interruption (partition/heal) simulations
(`crates/auradb-replication/tests/multi_node.rs`), plus recovery diagnostics
(`crates/auradb-cli/tests/cluster_diagnostics.rs`). The connector leader-hint
review was again **docs-only** (Option A): the `not_leader` contract is unchanged
and still pinned by `crates/auradb-server/tests/not_leader.rs`. The only wire
change is the additive `leader_changes` field on the cluster health report, which
the 0.3.x connector ignores — **no connector release is required** and AWP stays
at v1. Published Aura Connector 0.3.0 conformance remains a required CI gate
(`conformance.yml`).

### v0.7.0 connector cluster ergonomics

v0.7.0 is a coordinated server + connector release (Aura Connector v0.4.0). The
`not_leader` error frame gains an additive, structured `not_leader` object (leader
client address, leader/current node ids, term, role, and a usable `leader_hint`),
built from the node's current cluster view; fields are present only when known and
carry no secrets. The wire shape is covered by `auradb-protocol` unit tests, the
populated case by `crates/auradb-server/tests/cluster_preview.rs`
(`not_leader_payload_includes_leader_client_addr_when_known`,
`not_leader_payload_contains_no_secrets`), and the authenticated-session path by
`crates/auradb-server/tests/not_leader.rs` (`not_leader_payload_safe_over_tls_auth`).

A connector-driven cluster conformance runner,
`tests/conformance/python/run_connector_cluster.py`, drives Aura Connector v0.4.x
against a live preview cluster: a leader write, a follower `not_leader` exposing
the leader address, the reconnect helper, the bounded redirect helper, and a
transaction that is never auto-redirected. It is gated on
`AURADB_CLUSTER_LEADER_DSN` / `AURADB_CLUSTER_FOLLOWER_DSN` (with optional
`AURADB_CLUSTER_TOKEN` / `AURADB_CLUSTER_CA`) and skips cleanly otherwise. AWP
stays at v1; Aura Connector 0.3.x remains compatible (it ignores the new object
and routes the leader manually).

### v0.7.1 connector ergonomics polish (Aura Connector v0.4.1)

v0.7.1 is a coordinated patch with **Aura Connector v0.4.1**. The server and the
`not_leader` payload are unchanged from v0.7.0 (byte-for-byte), so the
compatibility tests above still pin the contract; v0.4.1 only improves the
client-side experience (clearer `AuraNotLeaderError` messages, secure-by-default
redirect, transaction-redirect docs).

**Connector selection in CI.** The `cluster.yml` loopback job installs the
published connector in the `>=0.4.1,<0.5` line and, when it is available, runs
three scenarios against the live cluster: (1) `run_connector_smoke.py` against the
leader, (2) `run_connector_conformance.py` against the leader, and (3)
`run_connector_cluster.py` across leader + follower (`AuraNotLeaderError` message
carries the leader address; reconnect helper works; redirect is bounded;
transaction redirect is rejected; auth/TLS preserved when configured).

- **On PR/push**, if the published connector is not installable yet (or reports it
  is too old), the step **skips with a clear message** rather than failing — so
  coordinated work does not block on connector publish timing. We do not claim a
  connector version is published when it is not.
- **On release/tag conformance**, run `cluster.yml` via *Run workflow* with the
  `require_published_connector` input set so a missing/too-old connector **fails**
  the job. Flip it on only once Aura Connector v0.4.1 is published.

### v0.9.0 connector behavior under leader change (HA release candidate)

v0.9.0 adds a leader-change conformance scenario,
`tests/conformance/python/run_connector_leader_change.py`, for the controlled
static-cluster preview (an **HA release candidate, not production HA**). Run it
against a live cluster **after** a leader change (for example from
`scripts/smoke_ha_candidate.sh`, which stops the leader, waits for a new one, and
then invokes the script). It confirms, with the published Aura Connector v0.4.1:

- the old leader no longer accepts a leader write (it is down or demoted) — a
  single bounded attempt, no infinite retry;
- the client discovers the new leader from the `not_leader` hint or by a bounded
  probe of the candidate addresses;
- a write to the new leader succeeds;
- `connect_to_leader` and the bounded `with_leader_redirect` reach the new
  leader, preserving auth and TLS across the redirect;
- a transaction is never auto-redirected across a leader change.

Invoke it directly with:

```bash
python tests/conformance/python/run_connector_leader_change.py \
  --leader <old-leader-client-addr> \
  --candidate-addrs <addr1,addr2,addr3>
```

It exits 0 on success, 1 on a failed check, and 2 if the connector is
missing/too old (so coordinated work does not block on connector publish timing).

### v0.9.1 leader-resolution path reporting (HA release-candidate stabilization)

v0.9.1 keeps the v0.9.0 scenario above and makes the new-leader discovery step
explicit: `run_connector_leader_change.py` now **reports the resolution path** it
took — whether it reached the new leader **directly from the `not_leader` hint**
(which now carries the leader's own client address when the leader sets
`[cluster] advertise_client_addr`) or fell back to a **bounded re-resolve probe**
of the candidate addresses. It prefers the hint and then probes. Both paths are
valid HA-candidate behavior: when the hint's client address is reachable from the
client it is used directly; when it is not (for example a Docker in-network
address that is not the host-published port), the documented re-resolve fallback
applies. The script's exit codes are unchanged.

### v0.9.2 final stabilization

v0.9.2 keeps the v0.9.0/v0.9.1 connector conformance scenarios unchanged (no wire,
config, or connector change) and remains validated against **Aura Connector
v0.4.1**. It adds Rust-side tests that pin the leader-hint contract across multiple
leader changes and an old-leader rejoin, and a docs-consistency test for the
in-network vs. host-published client-address explanation. Connector leader-change
behavior across **every** supported client remains an open v1.0 production-HA
criterion — see [V1_0_DECISION_CHECKLIST.md](V1_0_DECISION_CHECKLIST.md) §4.

### v1.0.1 (current release)

v1.0.1 is the first production patch on the v1.0 single-node production line, with
a multi-node HA candidate preview. It adds no wire, config, or connector change
over v1.0.0 — the Aura Wire Protocol stays at AWP 1 and the storage format stays at
v2 — so the v0.9.0/v0.9.1/v0.9.2 connector conformance scenarios above carry forward
unchanged and are still run against **Aura Connector v0.4.1** (and compatible
0.4.x). Connector leader-change behavior across **every** supported client
remains an open production-HA criterion for the multi-node preview — see
[V1_0_DECISION_CHECKLIST.md](V1_0_DECISION_CHECKLIST.md) §4.

## Running

Rust (no server needed - the test spawns one):

```bash
cargo test -p auradb-conformance
```

Python (against a running server):

```bash
cargo run --release -p auradb-cli -- server --data-dir .local/auradb --port 7171 &
python tests/conformance/python/run_conformance.py --addr 127.0.0.1:7171
```

Python against an authenticated, TLS-terminated server:

```bash
python tests/conformance/python/run_conformance.py --addr 127.0.0.1:7171 \
  --auth-token "your-secret" --tls-ca .local/certs/ca.crt
```

## Official-client harnesses

Two Python harnesses drive a running server through the published Aura Connector
and its native AuraDB backend:

- `tests/conformance/python/run_connector_smoke.py` - a minimal, fast scenario
  (connect, ping, auth, TLS, schema, insert, find, stream, read-your-writes
  transaction, vector nearest, full-text, document path, close).
- `tests/conformance/python/run_connector_conformance.py` - the full scenario
  suite.

Both accept `--auth-token`, `--tls-ca`, and `--tls-server-name`.

## CI

The `conformance.yml` workflow runs the standard-library Python harness against a
live server in three configurations (auth disabled, auth enabled with a rejection
check, and TLS), and runs the connector smoke (auth disabled and auth plus TLS)
and the full connector conformance against a freshly built server with the
published connector installed.

## Status

All Rust scenarios pass in CI via `cargo test`. The standard-library Python
harness passes all scenarios against a locally running server. The connector
harnesses were also validated locally with the published `aura-connector` 0.3.0
(installed from PyPI within `aura-connector>=0.3,<0.4`): the smoke passed in
plaintext, auth, and TLS-plus-auth modes (11/11 checks each), and the
standard-library Python wire conformance passed over TLS-plus-auth (17/17
scenarios), with no token, token hash, or private key appearing in the server
logs.

For the **v0.4.1** release the connector conformance gap was closed against the
**published** `aura-connector` 0.3.0 (installed from PyPI within
`aura-connector>=0.3,<0.4`; `aura.__version__ == "0.3.0"`), driven through its
native AuraDB backend against a freshly built v0.4.1 server in **both** supported
deployment modes:

- **Non-cluster (recommended) mode** — `run_connector_smoke.py` 12/12 checks and
  `run_conformance.py` 18/18 scenarios passed; the server's `health()` frame
  carries no `cluster` section and the connector handles it cleanly.
- **Single-node cluster mode** (`examples/auradb.cluster.local.toml`, writes
  routed through the Raft log) — `run_connector_smoke.py` 12/12 checks and
  `run_conformance.py` 18/18 scenarios passed. The additive `cluster` health
  section is present and honest (`single_node = true`, `peer_count = 0`,
  `applied_index == commit_index`, role `leader`) and the published 0.3.x
  connector ignores the unknown field without error.

`not_leader` was validated by the staged server-layer test
(`crates/auradb-server/tests/not_leader.rs`, 3/3 passing) plus a direct check of
the published connector's error mapping: the `not_leader` code is not modelled by
0.3.x, so it falls back to the generic `AuraServerError` (acceptable for v0.4.1),
arrives with `retryable = False` (the wire frame omits the field, which the
connector defaults to false), and the connector retry policy is bounded
(`max_attempts = 3`), so a client never retries forever. No connector change was
required.

## Official client

The published Aura Connector (>= 0.3.0) drives the same server through its native
AuraDB backend, including auth and TLS. The documented Query IR shapes
(`docs/QUERY_ENGINE.md`) describe the wire-level contract. See
[AURA_CONNECTOR_COMPATIBILITY.md](AURA_CONNECTOR_COMPATIBILITY.md) and
[COMPATIBILITY.md](COMPATIBILITY.md).

Now that Aura Connector 0.3.0 is published, the `connector` job in
`.github/workflows/conformance.yml` is **active and required**: CI installs
`aura-connector>=0.3.0` from PyPI and runs
`tests/conformance/python/run_connector_conformance.py` against a freshly built
server. It is no longer a no-op gated on the package being unavailable. A planned
enhancement (see [ROADMAP](ROADMAP.md)) pins golden frame and IR fixtures from the
connector package so conformance is checked against the canonical client encoding.
