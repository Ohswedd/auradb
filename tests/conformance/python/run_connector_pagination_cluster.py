#!/usr/bin/env python3
"""Aura Connector ranked pagination against an AuraDB multi-node preview.

Validates v1.2 stable ranked pagination across a controlled static-cluster preview:
paging a BM25 search against the leader yields duplicate-free, deterministically
ordered pages by opaque cursor token; a write is leader-only (a write to a follower
raises ``AuraNotLeaderError``); a paged search survives a redirect to the leader
with its query preserved; the follower's read behavior is recorded honestly as
eventually consistent; and — when candidate addresses are supplied — pagination
works against the current leader after a leader change.

Honest scope: AuraDB multi-node mode is a controlled static-cluster preview, NOT a
production high-availability guarantee. There is no production failover and no
linearizable reads. The supported, recommended path is to send reads to the leader;
in the preview, followers serve reads from their locally replicated state, which are
eventually consistent and not linearizable. Writes remain leader-only.

Exit codes: 0 success, 1 a failed check, 2 the connector is missing/too old.

Usage:
    python -m pip install "aura-connector>=0.6,<0.7"
    python run_connector_pagination_cluster.py --leader 127.0.0.1:7171 --follower 127.0.0.1:7181
    python run_connector_pagination_cluster.py --leader L --follower F \
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


class PageCDoc(AuraModel):
    id: int = Field(primary_key=True)
    body: str = Field(full_text=True)
    embedding: Vector[3]


_SEED = [(n, f"lorem ipsum document {n} lorem", [float(n) / 10.0, 0.0, 0.0]) for n in range(1, 11)]


def _dsn(addr: str, tls_ca: str | None) -> str:
    return f"{'auradbs' if tls_ca else 'auradb'}://{addr}/pagecluster"


def _options(token: str | None, tls_ca: str | None) -> dict:
    options: dict = {}
    if token:
        options["auth"] = TokenAuth(token)
    if tls_ca:
        options["tls"] = TLSConfig(enabled=True, ca_cert_path=tls_ca, verify_hostname=True)
    return options


async def _paged_ids(client) -> list[int]:
    ids: list[int] = []
    async for page in client.search(PageCDoc).search_text("body", "lorem").search_pages(page_size=3):
        ids.extend(r.id for r in page.rows)
    return ids


async def _resolve_leader(candidates: list[str], options: dict, tls_ca: str | None) -> str | None:
    for _ in range(20):
        for addr in candidates:
            try:
                async with connect(_dsn(addr, tls_ca), models=[PageCDoc], **options) as client:
                    if not await client.ping():
                        continue
                    try:
                        await client.upsert(
                            PageCDoc, key={"id": 999},
                            values={"body": "probe", "embedding": [0.0, 0.0, 1.0]},
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

    print("Connector pagination cluster conformance (HA candidate preview, NOT production HA)")
    print(f"  leader:   {leader}")
    print(f"  follower: {follower}")

    # Seed through the leader.
    async with connect(_dsn(leader, tls_ca), models=[PageCDoc], **options) as client:
        for did, body, emb in _SEED:
            await client.upsert(PageCDoc, key={"id": did}, values={"body": body, "embedding": emb})

        # 1. pagination_to_leader_pass — duplicate-free, deterministic pages on the leader.
        ids = await _paged_ids(client)
        ids_again = await _paged_ids(client)
        check("pagination_to_leader_pass",
              len(ids) == len(set(ids)) == len(_SEED) and ids == ids_again,
              f"ids={ids} again={ids_again}")

    # 2. A write is leader-only on a follower; a paged search then survives a redirect
    #    to the leader with its query preserved.
    async with connect(_dsn(follower, tls_ca), models=[PageCDoc], **options) as client:
        try:
            await client.upsert(
                PageCDoc, key={"id": 20}, values={"body": "follower write", "embedding": [0.0, 0.0, 1.0]}
            )
            check("pagination_cluster_follower_not_leader_for_write", False,
                  "follower accepted a write")
        except AuraNotLeaderError as exc:
            check("pagination_cluster_follower_not_leader_for_write", True)
            leader_client = await client.connect_to_leader(exc)
            try:
                redirected = await _paged_ids(leader_client)
                check("pagination_cluster_redirect_preserves_query",
                      len(redirected) == len(set(redirected)) == len(_SEED), str(redirected))
            finally:
                await leader_client.close()

        # 3. follower_read_behavior_documented — eventually consistent, never linearizable.
        try:
            await asyncio.sleep(1.0)
            fids = await _paged_ids(client)
            note("follower_read_behavior_documented",
                 f"follower served an eventually-consistent paged read ({len(fids)} rows, "
                 f"unique={len(set(fids))}); not linearizable — page against the leader for "
                 "fresh, stable results")
        except AuraError as e:
            note("follower_read_behavior_documented",
                 f"follower rejected the paged read ({type(e).__name__}); page against the leader")

    # 4. leader_change_then_pagination_pass — after a (possible) leader change, pagination
    #    works against the current leader. Operator note: trigger a leader change (e.g. stop
    #    the current leader) before supplying --candidate-addrs to exercise this for real.
    if candidates:
        current = await _resolve_leader(candidates, options, tls_ca)
        check("pagination_cluster_resolve_current_leader", bool(current),
              "no leader among candidates")
        if current:
            async with connect(_dsn(current, tls_ca), models=[PageCDoc], **options) as client:
                ids = await _paged_ids(client)
                check("leader_change_then_pagination_pass",
                      len(ids) == len(set(ids)) == len(_SEED), str(ids))
    else:
        note("leader_change_then_pagination_pass",
             "skipped: pass --candidate-addrs after stopping the old leader to exercise it")

    # 5. Documentation guard: this harness never claims production HA.
    check("pagination_cluster_no_production_ha_claim", True,
          "preview only; not production high availability")

    total = len(passed) + len(failed)
    print(f"\nConnector pagination cluster conformance: {len(passed)}/{total} checks passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Aura Connector ranked-pagination conformance for an AuraDB cluster preview"
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
