# AuraDB v1.3.1 release notes

**Release-smoke correctness — single-node production line, multi-node HA candidate
preview.**

AuraDB v1.3.1 is a patch release for release-smoke correctness. It includes the fixed
`scripts/smoke_cluster_search_analytics.sh` from PR #39. There are **no** engine,
protocol, storage, query, connector, or compatibility behavior changes. AWP remains 1.
Storage format remains v2. Aura Connector v0.7.0 remains the tested connector line.
Single-node remains production-supported. Multi-node remains an HA candidate preview,
**not** production HA. HNSW/ANN remains an opt-in preview, **not** production ANN.

The v1.3.0 tag is **not** moved; v1.3.1 is cut as a separate patch. No Aura Connector
release is needed.

## Why this patch

A post-release issue was found in the cluster search-analytics release smoke (the
engine and the published image were already correct — this was a smoke/test-harness bug
only):

- The smoke resolved the cluster leader by grepping the first `127.0.0.1:<port>` token,
  which could match the queried seed's own address.
- It hardcoded/biased the `7171` (node1) client port, so the run could fail whenever
  node2 or node3 won the Docker Compose leader election.
- The bounded failover drill could accept a **stale stopped leader**: right after the
  leader is stopped, a survivor can still name the dead node as leader until its
  election timeout fires.

## What changed in v1.3.1

- **Version bump** to `1.3.1` across the workspace (`Cargo.toml`, `Cargo.lock`), the CLI
  (`auradb version` / `auradb compatibility`, which continues to report `Aura Connector
  (tested): 0.7.0`), the documentation, and the Docker Compose image tags.
- **Fixed cluster search-analytics release smoke** (`scripts/smoke_cluster_search_analytics.sh`):
  - **Leader resolution by self-reported role.** `find_leader_addr` polls each node's
    own `cluster status` and selects the host port whose node reports `role=Leader`,
    rather than trusting a `leader_client_addr` hint or grepping an address token that
    could match the queried seed.
  - **Genuine leader-change drill.** The drill stops the current leader, then waits
    until a different reachable node reports `role=Leader` while excluding the stopped
    port, so a stale survivor still pointing at the dead node is rejected. The
    search/facet/pagination/group-by checks then re-run under the new leader, and quorum
    is restored after the old node rejoins.
  - **Portable lookups.** Plain `case` port↔service lookups (no associative arrays) keep
    the script runnable on stock macOS bash 3.2.
  - The script header continues to state that this is the experimental multi-node
    preview and an HA *candidate* drill — **not production HA proof** — on a controlled
    single-host cluster.

## Compatibility

- **Storage format unchanged** (manifest `format_version` 2). Data directories from
  every prior v0.3.x–v1.3.x release open in place; no migration is required.
- **Aura Wire Protocol unchanged** (AWP 1). No new request or response shapes.
- **Aura Connector v0.7.0 paired** (and compatible 0.7.x). Connector v0.6.x remains
  supported for existing features and is backward compatible with v0.6.1. **No connector
  release is required for v1.3.1.**
- **Configuration is backward compatible.** No new config fields; no config flag changes
  meaning.
- All v1.3.0 behavior is preserved byte-for-byte.

The frozen v1 compatibility surfaces are unchanged: **Aura Wire Protocol 1** is the
stable v1 wire protocol and **storage format v2** is the stable v1 single-node storage
format, each preserved across v1.x unless a security, correctness, safety, or corruption
issue requires a documented change or migration. See [`COMPATIBILITY.md`](COMPATIBILITY.md),
[`PROTOCOL.md`](PROTOCOL.md), [`STORAGE_ENGINE.md`](STORAGE_ENGINE.md), and
[`AURA_CONNECTOR_COMPATIBILITY.md`](AURA_CONNECTOR_COMPATIBILITY.md).

## What did not change

- No engine, query, storage, replication, or protocol behavior change. The GROUP BY
  aggregations, EXPLAIN ANALYZE query-profile fields, durable approximate-vector preview
  metadata, the `auradb vector eval` harness, and all earlier v1.x query features shipped
  in v1.3.0 and are unchanged here.
- No public connector API change, and no new Aura Connector release.
- No production HA claim and no production ANN claim. Exact vector search remains the
  default and the correctness baseline.

## Known limitations

Honest limitations carried by this release (unchanged scope boundaries):

- **Multi-node is an HA candidate preview, not production HA.** No production automatic
  failover, no linearizable follower reads (follower reads/search are eventually
  consistent), no distributed transactions, and no dynamic membership, sharding, or
  multi-region. Single-node remains the recommended production mode.
- **Approximate (HNSW) vector search is an opt-in preview, not production ANN.** The
  graph is in-memory and rebuilt from the exact vectors (never persisted; not
  incremental). Exact vector search remains the default and the correctness baseline.
- **The cluster search-analytics smoke is a controlled single-host preview drill.** It
  exercises a static three-node Compose cluster on one host and is **not** production HA
  proof.
