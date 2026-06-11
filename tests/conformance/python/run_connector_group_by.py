#!/usr/bin/env python3
"""AuraDB GROUP BY conformance harness driven by the Aura Connector.

Exercises a running AuraDB v1.3.x server's single-field GROUP BY aggregations
over the wire through the public Aura Connector v0.7.x query API
(``QueryBuilder.group_by`` plus per-group ``aggregate_count`` / ``min`` / ``max``),
including filter scoping, BM25 search-candidate scoping, deterministic
count-desc / key-asc group ordering, and ``group_limit`` truncation with an
honest ``group_count_total``.

It connects to a live server (never the in-memory reference backend) and is
idempotent across re-runs: it upserts a fixed dataset under a uniquely named
collection so the counts are stable.

Usage:
    python -m pip install "aura-connector>=0.7,<0.8"   # or: pip install -e ../aura-connector
    python run_connector_group_by.py --addr 127.0.0.1:7171
    python run_connector_group_by.py --addr 127.0.0.1:7171 --auth-token dev-secret \
        --tls-ca .local/certs/ca.crt --tls-server-name localhost

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

from _conformance_isolation import add_isolation_args, collection_prefix, scoped_models

try:
    from aura import AuraCapabilityError, AuraModel, Field, Vector, connect
    from aura.config import TLSConfig, TokenAuth
except ImportError:  # pragma: no cover
    print("aura-connector >= 0.7 is required: pip install 'aura-connector>=0.7,<0.8'")
    sys.exit(2)


class ConfGroupItem(AuraModel):
    id: str = Field(primary_key=True)
    category: str = Field(index=True)
    brand: str
    body: str = Field(full_text=True)
    price: int
    embedding: Vector[3]


# Fixed dataset. category counts: alpha=3, beta=2, gamma=2 (tie), delta=1 (total 8).
# brand counts: acme=4, bolt=4 (tie -> key asc), cusp=... designed for limit/tie tests.
_DATASET = [
    ("i1", "alpha", "acme", "raft consensus raft", 10, [1.0, 0.0, 0.0]),
    ("i2", "alpha", "acme", "raft module coordinates", 20, [0.9, 0.1, 0.0]),
    ("i3", "alpha", "bolt", "storage compaction", 30, [0.0, 1.0, 0.0]),
    ("i4", "beta", "bolt", "raft leader election", 40, [0.8, 0.2, 0.0]),
    ("i5", "beta", "acme", "indexing statistics", 50, [0.0, 0.0, 1.0]),
    ("i6", "gamma", "bolt", "vector search baseline", 60, [0.1, 0.0, 0.9]),
    ("i7", "gamma", "acme", "hybrid retrieval fusion", 70, [0.2, 0.0, 0.8]),
    ("i8", "delta", "bolt", "backup and restore", 80, [0.0, 0.5, 0.5]),
]


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str, prefix: str) -> int:
    (ConfGroupItem,) = scoped_models(prefix, globals()["ConfGroupItem"])
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/conf_group_by"
    options: dict = {}
    if token:
        options["auth"] = TokenAuth(token)
    if tls_ca:
        options["tls"] = TLSConfig(enabled=True, ca_cert_path=tls_ca, verify_hostname=True)

    passed: list[str] = []
    failed: list[str] = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        (passed if ok else failed).append(name)
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}" + ("" if ok else f": {detail}"))

    async with connect(dsn, models=[ConfGroupItem], **options) as client:
        # ----- group_by_insert_dataset -----
        for did, cat, brand, body, price, emb in _DATASET:
            await client.upsert(
                ConfGroupItem,
                key={"id": did},
                values={
                    "category": cat,
                    "brand": brand,
                    "body": body,
                    "price": price,
                    "embedding": emb,
                },
            )
        check("group_by_insert_dataset", True)

        # ----- group_by_capability_or_clear_error -----
        try:
            base = await client.query(ConfGroupItem).group_by("category").aggregate()
        except AuraCapabilityError as exc:
            check("group_by_capability_or_clear_error", False, f"capability absent: {exc}")
            print(f"\nConnector group_by conformance: {len(passed)}/{len(passed) + len(failed)} passed")
            return 1
        check("group_by_capability_or_clear_error", base.groups is not None)

        # ----- group_by_count_basic (count-desc, key-asc) -----
        g = base.groups
        seq = [(b.key, b.count) for b in g.groups] if g else []
        expected = [("alpha", 3), ("beta", 2), ("gamma", 2), ("delta", 1)]
        check(
            "group_by_count_basic",
            g is not None and seq == expected and g.group_count_total == 4,
            f"{seq} total={getattr(g, 'group_count_total', None)}",
        )

        # ----- group_by_min_max_per_group -----
        mm = (
            await client.query(ConfGroupItem)
            .group_by("category")
            .min("price")
            .max("price")
            .aggregate()
        )
        alpha = mm.groups.group("alpha") if mm.groups else None
        check(
            "group_by_min_max_per_group",
            alpha is not None
            and alpha.metric("min", "price") == 10
            and alpha.metric("max", "price") == 30,
            f"alpha min={alpha and alpha.metric('min', 'price')} "
            f"max={alpha and alpha.metric('max', 'price')}",
        )

        # ----- group_by_limit_and_truncated -----
        limited = await client.query(ConfGroupItem).group_by("category", limit=2).aggregate()
        lg = limited.groups
        check(
            "group_by_limit_and_truncated",
            lg is not None
            and len(lg.groups) == 2
            and lg.group_limit == 2
            and lg.group_count_total == 4
            and lg.truncated,
            f"returned={lg and len(lg.groups)} total={lg and lg.group_count_total} "
            f"truncated={lg and lg.truncated}",
        )

        # ----- group_by_with_filter -----
        filt = (
            await client.query(ConfGroupItem)
            .where(ConfGroupItem.price >= 40)
            .group_by("category")
            .aggregate()
        )
        # prices >= 40: i4(beta), i5(beta), i6(gamma), i7(gamma), i8(delta)
        counts = {b.key: b.count for b in filt.groups.groups} if filt.groups else {}
        check(
            "group_by_with_filter",
            filt.filter_present
            and counts.get("beta") == 2
            and counts.get("gamma") == 2
            and counts.get("delta") == 1
            and "alpha" not in counts,
            f"filter_present={filt.filter_present} counts={counts}",
        )

        # ----- group_by_search_scoped -----
        # GROUP BY over the BM25 "raft" candidate set (i1,i2,i4 -> alpha x2, beta x1).
        scoped = (
            await client.query(ConfGroupItem)
            .search_text("body", "raft")
            .group_by("category")
            .aggregate()
        )
        sc = {b.key: b.count for b in scoped.groups.groups} if scoped.groups else {}
        check(
            "group_by_search_scoped",
            scoped.search_scoped and sc.get("alpha") == 2 and sc.get("beta") == 1,
            f"search_scoped={scoped.search_scoped} counts={sc}",
        )

    total = len(passed) + len(failed)
    print(f"\nConnector group_by conformance: {len(passed)}/{total} passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="AuraDB connector GROUP BY conformance")
    parser.add_argument("--addr", default="127.0.0.1:7171")
    parser.add_argument("--auth-token", default=None)
    parser.add_argument("--tls-ca", default=None)
    parser.add_argument("--tls-server-name", default="localhost")
    add_isolation_args(parser)
    args = parser.parse_args()
    prefix = collection_prefix(args)
    sys.exit(
        asyncio.run(run(args.addr, args.auth_token, args.tls_ca, args.tls_server_name, prefix))
    )


if __name__ == "__main__":
    main()
