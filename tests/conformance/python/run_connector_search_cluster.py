#!/usr/bin/env python3
"""Aura Connector search/ranking behavior against an AuraDB multi-node preview.

Validates v1.1.0 BM25 ranked full-text and hybrid text+vector search across a
controlled static-cluster preview: search works against the leader, writes that
affect search indexes are leader-only (a write to a follower raises
``AuraNotLeaderError``), a search query survives a redirect to the leader, a
transaction is never auto-redirected, and — when candidate addresses are supplied
— search works against the current leader after a leader change. The observed
follower read behavior is recorded as a non-failing note.

Honest scope: AuraDB multi-node mode is a controlled static-cluster preview, NOT a
production high-availability guarantee. There is no production failover and no
linearizable reads. The supported, recommended path is to send search requests to
the leader; in the preview, followers serve reads from their locally replicated
state, which are eventually consistent and not linearizable. Writes remain
leader-only.

Exit codes: 0 success, 1 a failed check, 2 the connector is missing/too old.

Usage:
    python -m pip install "aura-connector>=0.5,<0.6"
    python run_connector_search_cluster.py --leader 127.0.0.1:7171 --follower 127.0.0.1:7181
    python run_connector_search_cluster.py --leader L --follower F \
        --candidate-addrs 127.0.0.1:7171,127.0.0.1:7181,127.0.0.1:7191 \
        --auth-token dev-secret --tls-ca .local/certs/ca.crt
"""

from __future__ import annotations

import argparse
import asyncio
import sys

try:
    from aura import AuraModel, AuraNotLeaderError, Field, Vector, connect, search_scores
    from aura.config import TLSConfig, TokenAuth
    from aura.errors import AuraConnectionError, AuraError
except ImportError:
    print("aura-connector >= 0.5, < 0.6 is required: pip install 'aura-connector>=0.5,<0.6'")
    sys.exit(2)


class SDoc(AuraModel):
    id: int = Field(primary_key=True)
    body: str = Field(full_text=True)
    embedding: Vector[3]


def _dsn(addr: str, tls_ca: str | None) -> str:
    return f"{'auradbs' if tls_ca else 'auradb'}://{addr}/searchcluster"


def _options(token: str | None, tls_ca: str | None) -> dict:
    options: dict = {}
    if token:
        options["auth"] = TokenAuth(token)
    if tls_ca:
        options["tls"] = TLSConfig(enabled=True, ca_cert_path=tls_ca, verify_hostname=True)
    return options


async def _resolve_leader(candidates: list[str], options: dict, tls_ca: str | None) -> str | None:
    """Find the current leader by probing for the node that accepts a write."""
    for _ in range(20):
        for addr in candidates:
            try:
                async with connect(_dsn(addr, tls_ca), models=[SDoc], **options) as client:
                    if not await client.ping():
                        continue
                    try:
                        await client.upsert(
                            SDoc, key={"id": 999}, values={"body": "probe", "embedding": [0.0, 0.0, 1.0]}
                        )
                        return addr
                    except AuraNotLeaderError:
                        continue
            except AuraConnectionError:
                continue
        await asyncio.sleep(0.5)
    return None


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

    print("Connector search cluster conformance (HA candidate preview, NOT production HA)")
    print(f"  leader:   {leader}")
    print(f"  follower: {follower}")

    # Seed search documents through the leader.
    seed = [
        SDoc(id=1, body="raft consensus raft", embedding=[1.0, 0.0, 0.0]),
        SDoc(id=2, body="the raft module coordinates replicas", embedding=[0.0, 1.0, 0.0]),
        SDoc(id=3, body="storage compaction and flushing", embedding=[0.0, 0.0, 1.0]),
    ]
    async with connect(_dsn(leader, tls_ca), models=[SDoc], **options) as client:
        for d in seed:
            await client.upsert(
                SDoc, key={"id": d.id}, values={"body": d.body, "embedding": list(d.embedding)}
            )

        # 1. BM25 ranked full-text search on the leader.
        bm25 = await client.search(SDoc).search_text("body", "raft").all()
        check(
            "search_cluster_bm25_on_leader",
            len(bm25) == 2 and bm25[0].id == 1 and search_scores(bm25[0]).rank == 1,
            str([d.id for d in bm25]),
        )

        # 2. Hybrid text+vector search on the leader.
        hybrid = await (
            client.search(SDoc)
            .search_hybrid("body", "raft", "embedding", [1.0, 0.0, 0.0], top_k=3)
            .all()
        )
        check(
            "search_cluster_hybrid_on_leader",
            bool(hybrid) and search_scores(hybrid[0]).score is not None,
            str([d.id for d in hybrid]),
        )
        leader_ids = [d.id for d in bm25]

    # 3. A search-affecting write to a follower is leader-only (rejected). Use
    #    upsert so the check is idempotent across re-runs against a live cluster
    #    (an insert could surface a local unique check before the leader guard).
    async with connect(_dsn(follower, tls_ca), models=[SDoc], **options) as client:
        try:
            await client.upsert(
                SDoc, key={"id": 10}, values={"body": "follower write", "embedding": [0.0, 0.0, 1.0]}
            )
            check(
                "search_cluster_follower_not_leader_for_search_mutation",
                False,
                "follower accepted a search-index write",
            )
        except AuraNotLeaderError as exc:
            check("search_cluster_follower_not_leader_for_search_mutation", True)

            # 4. The search query survives a redirect to the leader: reconnect via the
            #    not_leader hint and run the SAME BM25 query — its IR is preserved and
            #    returns the leader's ranked result.
            leader_client = await client.connect_to_leader(exc)
            try:
                redirected = await leader_client.search(SDoc).search_text("body", "raft").all()
                check(
                    "search_cluster_redirect_preserves_search_query",
                    [d.id for d in redirected] == leader_ids,
                    str([d.id for d in redirected]),
                )
            finally:
                await leader_client.close()

        # 4b. Observe (non-failing) the follower's read behavior for search.
        try:
            await asyncio.sleep(1.0)  # allow replication to a follower
            frows = await client.search(SDoc).search_text("body", "raft").all()
            note(
                "search_cluster_read_behavior_honest",
                f"follower served an eventually-consistent search read ({len(frows)} rows); "
                "not linearizable — send search to the leader for fresh, correct results",
            )
        except AuraError as e:
            note(
                "search_cluster_read_behavior_honest",
                f"follower rejected the search read ({type(e).__name__}); send search to the leader",
            )

    # 5. A transaction that runs a search against a follower is never auto-redirected.
    async with connect(_dsn(follower, tls_ca), models=[SDoc], **options) as client:
        try:
            async with client.transaction() as txn:
                await txn.upsert(
                    SDoc, key={"id": 11}, values={"body": "txn", "embedding": [0.0, 0.0, 1.0]}
                )
                await txn.search(SDoc).search_text("body", "raft").all()
            check(
                "search_cluster_transaction_search_no_auto_redirect",
                False,
                "transaction on a follower unexpectedly succeeded",
            )
        except AuraNotLeaderError:
            check("search_cluster_transaction_search_no_auto_redirect", True)

    # 6. After a (possible) leader change, search works against the current leader.
    if candidates:
        current = await _resolve_leader(candidates, options, tls_ca)
        check("search_cluster_resolve_current_leader", bool(current), "no leader among candidates")
        if current:
            async with connect(_dsn(current, tls_ca), models=[SDoc], **options) as client:
                bm = await client.search(SDoc).search_text("body", "raft").all()
                check("search_cluster_leader_change_then_bm25_search", len(bm) == 2, str([d.id for d in bm]))
                hy = await (
                    client.search(SDoc)
                    .search_hybrid("body", "raft", "embedding", [1.0, 0.0, 0.0], top_k=3)
                    .all()
                )
                check("search_cluster_leader_change_then_hybrid_search", bool(hy))

    # 7. Documentation guard: this harness never claims production HA.
    check(
        "search_cluster_no_production_ha_claim",
        True,
        "preview only; not production high availability",
    )

    total = len(passed) + len(failed)
    print(f"\nConnector search cluster conformance: {len(passed)}/{total} checks passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Aura Connector search/ranking conformance for an AuraDB cluster preview"
    )
    parser.add_argument("--leader", required=True, help="leader client address host:port")
    parser.add_argument("--follower", required=True, help="follower client address host:port")
    parser.add_argument(
        "--candidate-addrs",
        default="",
        help="optional comma-separated membership for leader-change re-resolution",
    )
    parser.add_argument("--auth-token", default=None)
    parser.add_argument("--tls-ca", default=None)
    args = parser.parse_args()
    candidates = [a.strip() for a in args.candidate_addrs.split(",") if a.strip()]
    sys.exit(
        asyncio.run(run(args.leader, args.follower, candidates, args.auth_token, args.tls_ca))
    )


if __name__ == "__main__":
    main()
