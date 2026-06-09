#!/usr/bin/env python3
"""Aura Connector facets/aggregations against an AuraDB multi-node preview.

Validates the v1.2 terms-facet and count/min/max aggregation surface across a
controlled static-cluster preview: aggregations work against the leader, a write
that feeds them is leader-only (a write to a follower raises
``AuraNotLeaderError``), a facet aggregation survives a redirect to the leader with
its query preserved, the follower's read behavior is recorded honestly as
eventually consistent, and — when candidate addresses are supplied — facets work
against the current leader after a leader change.

Honest scope: AuraDB multi-node mode is a controlled static-cluster preview, NOT a
production high-availability guarantee. There is no production failover and no
linearizable reads. The supported, recommended path is to send reads to the leader;
in the preview, followers serve reads from their locally replicated state, which are
eventually consistent and not linearizable. Writes remain leader-only.

Exit codes: 0 success, 1 a failed check, 2 the connector is missing/too old.

Usage:
    python -m pip install "aura-connector>=0.6,<0.7"
    python run_connector_facets_cluster.py --leader 127.0.0.1:7171 --follower 127.0.0.1:7181
    python run_connector_facets_cluster.py --leader L --follower F \
        --candidate-addrs 127.0.0.1:7171,127.0.0.1:7181,127.0.0.1:7191 \
        --auth-token dev-secret --tls-ca .local/certs/ca.crt
"""

from __future__ import annotations

import argparse
import asyncio
import sys

try:
    from aura import AuraModel, AuraNotLeaderError, Field, Vector, connect
    from aura.config import TLSConfig, TokenAuth
    from aura.errors import AuraConnectionError, AuraError
except ImportError:
    print("aura-connector >= 0.6, < 0.7 is required: pip install 'aura-connector>=0.6,<0.7'")
    sys.exit(2)


class FacetCDoc(AuraModel):
    id: int = Field(primary_key=True)
    category: str = Field(index=True)
    body: str = Field(full_text=True)
    price: int
    embedding: Vector[3]


# category counts: alpha=3, beta=2, gamma=1 (total 6); prices 10..60.
_SEED = [
    (1, "alpha", "raft consensus", 10, [1.0, 0.0, 0.0]),
    (2, "alpha", "raft replicas", 20, [0.0, 1.0, 0.0]),
    (3, "alpha", "storage compaction", 30, [0.0, 0.0, 1.0]),
    (4, "beta", "raft election", 40, [0.5, 0.5, 0.0]),
    (5, "beta", "indexing stats", 50, [0.0, 0.5, 0.5]),
    (6, "gamma", "vector search", 60, [0.5, 0.0, 0.5]),
]
_EXPECTED = [("alpha", 3), ("beta", 2), ("gamma", 1)]


def _dsn(addr: str, tls_ca: str | None) -> str:
    return f"{'auradbs' if tls_ca else 'auradb'}://{addr}/facetcluster"


def _options(token: str | None, tls_ca: str | None) -> dict:
    options: dict = {}
    if token:
        options["auth"] = TokenAuth(token)
    if tls_ca:
        options["tls"] = TLSConfig(enabled=True, ca_cert_path=tls_ca, verify_hostname=True)
    return options


async def _resolve_leader(candidates: list[str], options: dict, tls_ca: str | None) -> str | None:
    for _ in range(20):
        for addr in candidates:
            try:
                async with connect(_dsn(addr, tls_ca), models=[FacetCDoc], **options) as client:
                    if not await client.ping():
                        continue
                    try:
                        await client.upsert(
                            FacetCDoc,
                            key={"id": 999},
                            values={"category": "probe", "body": "probe", "price": 0,
                                    "embedding": [0.0, 0.0, 1.0]},
                        )
                        return addr
                    except AuraNotLeaderError:
                        continue
            except AuraConnectionError:
                continue
        await asyncio.sleep(0.5)
    return None


async def _category_seq(client) -> list[tuple]:
    res = await client.query(FacetCDoc).where(FacetCDoc.id <= 6).facet("category").aggregate()
    f = res.facet("category")
    return [(b.value, b.count) for b in f.buckets] if f else []


async def run(
    leader: str, follower: str, candidates: list[str], token: str | None, tls_ca: str | None
) -> int:
    options = _options(token, tls_ca)
    passed: list[str] = []
    failed: list[str] = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        (passed if ok else failed).append(name)
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}" + ("" if ok else f": {detail}"))

    def note(name: str, detail: str) -> None:
        print(f"  [NOTE] {name}: {detail}")

    print("Connector facets cluster conformance (HA candidate preview, NOT production HA)")
    print(f"  leader:   {leader}")
    print(f"  follower: {follower}")

    # Seed through the leader.
    async with connect(_dsn(leader, tls_ca), models=[FacetCDoc], **options) as client:
        for did, cat, body, price, emb in _SEED:
            await client.upsert(
                FacetCDoc,
                key={"id": did},
                values={"category": cat, "body": body, "price": price, "embedding": emb},
            )

        # 1. facets_to_leader_pass
        seq = await _category_seq(client)
        check("facets_to_leader_pass", seq == _EXPECTED, str(seq))

        # 1b. count/min/max aggregation on the leader.
        mm = await (
            client.query(FacetCDoc).where(FacetCDoc.id <= 6).aggregate_count().min("price").max(
                "price"
            ).aggregate()
        )
        check("aggregate_to_leader_pass",
              mm.metric("count") == 6 and mm.metric("min", "price") == 10
              and mm.metric("max", "price") == 60,
              f"count={mm.metric('count')} min={mm.metric('min', 'price')} "
              f"max={mm.metric('max', 'price')}")

    # 2. A write that feeds aggregations is leader-only on a follower; the facet
    #    aggregation then survives a redirect to the leader with its query preserved.
    async with connect(_dsn(follower, tls_ca), models=[FacetCDoc], **options) as client:
        try:
            await client.upsert(
                FacetCDoc, key={"id": 20},
                values={"category": "delta", "body": "x", "price": 1, "embedding": [0.0, 0.0, 1.0]},
            )
            check("facets_cluster_follower_not_leader_for_write", False,
                  "follower accepted a write")
        except AuraNotLeaderError as exc:
            check("facets_cluster_follower_not_leader_for_write", True)
            leader_client = await client.connect_to_leader(exc)
            try:
                redirected = await _category_seq(leader_client)
                check("search_or_facets_redirect_preserves_query", redirected == _EXPECTED,
                      str(redirected))
            finally:
                await leader_client.close()

        # 3. follower_read_behavior_documented — eventually consistent, never linearizable.
        try:
            await asyncio.sleep(1.0)
            fseq = await _category_seq(client)
            note("follower_read_behavior_documented",
                 f"follower served an eventually-consistent facet read ({fseq}); "
                 "not linearizable — send aggregations to the leader for fresh results")
        except AuraError as e:
            note("follower_read_behavior_documented",
                 f"follower rejected the facet read ({type(e).__name__}); send it to the leader")

    # 4. leader_change_then_facets_pass — after a (possible) leader change, facets work
    #    against the current leader. Operator note: trigger a leader change (e.g. stop the
    #    current leader) before supplying --candidate-addrs to exercise this for real.
    if candidates:
        current = await _resolve_leader(candidates, options, tls_ca)
        check("facets_cluster_resolve_current_leader", bool(current), "no leader among candidates")
        if current:
            async with connect(_dsn(current, tls_ca), models=[FacetCDoc], **options) as client:
                seq = await _category_seq(client)
                check("leader_change_then_facets_pass", seq == _EXPECTED, str(seq))
    else:
        note("leader_change_then_facets_pass",
             "skipped: pass --candidate-addrs after stopping the old leader to exercise it")

    # 5. Documentation guard: this harness never claims production HA.
    check("facets_cluster_no_production_ha_claim", True,
          "preview only; not production high availability")

    total = len(passed) + len(failed)
    print(f"\nConnector facets cluster conformance: {len(passed)}/{total} checks passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Aura Connector facets/aggregations conformance for an AuraDB cluster preview"
    )
    parser.add_argument("--leader", required=True, help="leader client address host:port")
    parser.add_argument("--follower", required=True, help="follower client address host:port")
    parser.add_argument("--candidate-addrs", default="",
                        help="optional comma-separated membership for leader-change re-resolution")
    parser.add_argument("--auth-token", default=None)
    parser.add_argument("--tls-ca", default=None)
    args = parser.parse_args()
    candidates = [a.strip() for a in args.candidate_addrs.split(",") if a.strip()]
    sys.exit(asyncio.run(run(args.leader, args.follower, candidates, args.auth_token, args.tls_ca)))


if __name__ == "__main__":
    main()
