# AuraDB v1.2.1 release notes

**Conformance and documentation hardening — single-node production line, multi-node HA
candidate preview.**

AuraDB v1.2.1 is a hardening patch on the v1.2 single-node production line. It is a
**conformance and documentation** release: it adds live over-the-wire conformance
scripts that exercise the v1.2 features (facets, aggregations, ranked pagination, and
cooperative query timeouts) end-to-end through the Aura Connector, wires them into the
conformance workflow, and refreshes the support and production documentation so it
enumerates the v1.2 feature set honestly.

It carries forward **all** v1.2.0 behavior. It adds **no** new database or query
features, changes **no** semantics, and touches **no** on-disk or wire format.
**Single-node mode remains the recommended production mode.** Multi-node static
clustering remains an **HA candidate preview** — strong release-candidate evidence,
but **not** a production HA guarantee, no production automatic failover, no production
cluster readiness.

## Compatibility

- **Storage format unchanged** (manifest `format_version` 2). Data directories from
  every prior v0.3.x–v1.2.x release open in place; no migration is required.
- **Aura Wire Protocol unchanged** (AWP 1). No new request or response shapes; the
  conformance scripts exercise only the existing v1.2 Query IR (the `aggregate` and
  `search_page` reads and the per-query `timeout_ms` field).
- **Aura Connector v0.6.1 paired** (and compatible 0.6.x). Connector v0.6.0 remains
  supported; v0.5.x remains supported for the pre-1.2 feature set. v0.6.1 is itself a
  conformance/documentation hardening release with no API change.
- **Configuration is backward compatible.** No new config fields are added; no config
  flag changes meaning.
- All v1.2.0 behavior is preserved.

The frozen v1 compatibility surfaces are unchanged: **Aura Wire Protocol 1** is the
stable v1 wire protocol and **storage format v2** is the stable v1 single-node storage
format, each preserved across v1.x unless a security, correctness, safety, or
corruption issue requires a documented change or migration. See
[`COMPATIBILITY.md`](COMPATIBILITY.md), [`PROTOCOL.md`](PROTOCOL.md),
[`STORAGE_ENGINE.md`](STORAGE_ENGINE.md), and
[`AURA_CONNECTOR_COMPATIBILITY.md`](AURA_CONNECTOR_COMPATIBILITY.md).

## What changed in v1.2.1

- **Version bump** to `1.2.1` across the workspace, CLI (`auradb version` /
  `auradb compatibility`, which now reports `Aura Connector (tested): 0.6.1`),
  documentation, and the Compose examples.
- **Live v1.2 conformance scripts** added under `tests/conformance/python/`, each
  driving a running server through the published Aura Connector API over the wire
  (never the in-memory backend):
  - `run_connector_facets.py` — terms facets (basic, limit, deterministic tie-break),
    `count`/`min`/`max` aggregations (all and filtered), and BM25-scoped facets, plus
    a clear capability error when a backend cannot serve them.
  - `run_connector_pagination.py` — ranked pagination by stable cursor token:
    duplicate-free pages across BM25, hybrid, and exact-vector ranking, cursor-token
    presence, structured invalid-cursor rejection, and the transaction-snapshot
    guidance for BM25/hybrid stability under concurrent writes.
  - `run_connector_timeouts.py` — per-query `timeout_ms` acceptance, the structured
    `query_timeout` error shape, that the connection survives a timeout and the next
    query succeeds, and that the cooperative nature is documented honestly.
  - Cluster variants `run_connector_facets_cluster.py`,
    `run_connector_pagination_cluster.py`, and `run_connector_timeouts_cluster.py`
    drive the same features against a leader, assert the same correctness, and
    document follower reads as **eventually consistent** (never linearizable).
- **CI wiring.** The conformance workflow runs the new single-node scripts against a
  live server using the paired connector wheel. The cluster variants are operator-run
  (their leader-change steps require stopping a node) and are documented as such in
  [`CONFORMANCE.md`](CONFORMANCE.md) and [`TESTING.md`](TESTING.md); they are not
  gated as required CI to avoid flaky required checks.
- **Documentation refresh.** [`SUPPORT_POLICY.md`](SUPPORT_POLICY.md) and
  [`PRODUCTION_READINESS.md`](PRODUCTION_READINESS.md) now enumerate the full v1.2
  single-node production feature set and clearly separate production-supported,
  preview, and explicitly unsupported capabilities.

## What did not change

- No new database or query features. The aggregations, terms facets, cooperative query
  timeouts, ranked pagination, and opt-in HNSW vector preview shipped in v1.2.0 and are
  unchanged here.
- No public connector API change. Pagination cursors continue to be managed internally
  by `QueryBuilder.search_pages(...)`; resuming a ranked page from an externally held
  cursor token is not part of the public API.
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
- **Query timeouts are cooperative, not preemptive.** Reads poll the deadline on their
  candidate/scan loop, so cancellation is "soon after" the deadline rather than
  instantaneous.
- **Ranked-pagination cursor stability under concurrent writes.** Vector cursors are
  duplicate-free across concurrent writes; BM25/hybrid ranked pagination is stable only
  when paged inside a transaction snapshot.
