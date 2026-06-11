#!/usr/bin/env python3
"""AuraDB query-profile conformance harness via the Aura Connector.

Exercises a running AuraDB v1.3.x server's best-effort EXPLAIN ANALYZE query
profile over the wire through the public Aura Connector v0.7.x API
(``QueryBuilder.profile()`` plus the advisory ``AggregateResult.profile``). The
profile is best-effort and additive: any or all fields may be absent, and an
older server omits it entirely. The contract verified here is that requesting a
profile never breaks a query, that the result is well typed when present, and
that the connector still exposes the client-side query IR via ``.explain()``.

Connects to a live server (never the in-memory reference backend); idempotent
across re-runs via upserts under a uniquely named collection.

Usage:
    python -m pip install "aura-connector>=0.7,<0.8"
    python run_connector_query_profile.py --addr 127.0.0.1:7171

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

from _conformance_isolation import add_isolation_args, collection_prefix, scoped_models

try:
    from aura import AuraModel, Field, QueryProfile, connect
    from aura.config import TLSConfig, TokenAuth
except ImportError:  # pragma: no cover
    print("aura-connector >= 0.7 is required: pip install 'aura-connector>=0.7,<0.8'")
    sys.exit(2)


class ConfProfileItem(AuraModel):
    id: str = Field(primary_key=True)
    category: str = Field(index=True)
    body: str = Field(full_text=True)
    price: int


_DATASET = [
    ("p1", "alpha", "raft consensus", 10),
    ("p2", "alpha", "raft replicas", 20),
    ("p3", "beta", "storage compaction", 30),
    ("p4", "beta", "indexing statistics", 40),
    ("p5", "gamma", "vector search", 50),
]


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str, prefix: str) -> int:
    (ConfProfileItem,) = scoped_models(prefix, globals()["ConfProfileItem"])
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/conf_query_profile"
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

    async with connect(dsn, models=[ConfProfileItem], **options) as client:
        # ----- profile_insert_dataset -----
        for did, cat, body, price in _DATASET:
            await client.upsert(
                ConfProfileItem,
                key={"id": did},
                values={"category": cat, "body": body, "price": price},
            )
        check("profile_insert_dataset", True)

        # ----- profile_request_does_not_break_query -----
        # Requesting a profile is additive: the aggregate still returns its result.
        result = (
            await client.query(ConfProfileItem)
            .where(ConfProfileItem.price >= 20)
            .aggregate_count()
            .profile()
            .aggregate()
        )
        check(
            "profile_request_does_not_break_query",
            result.metric("count") == 4 and result.filter_present,
            f"count={result.metric('count')}",
        )

        # ----- profile_typed_or_absent -----
        # The profile is best-effort: a QueryProfile when present, otherwise None.
        prof = result.profile
        check(
            "profile_typed_or_absent",
            prof is None or isinstance(prof, QueryProfile),
            f"profile type={type(prof).__name__}",
        )

        # ----- profile_fields_advisory_when_present -----
        # When a profile is present, its advisory fields are typed (or None); none
        # of them are load-bearing and the query succeeded regardless.
        if prof is not None:
            ok_fields = (
                (prof.rows_matched is None or isinstance(prof.rows_matched, int))
                and (prof.planning_us is None or isinstance(prof.planning_us, int))
                and (prof.execution_us is None or isinstance(prof.execution_us, int))
            )
            check("profile_fields_advisory_when_present", ok_fields, repr(prof))
        else:
            check("profile_fields_advisory_when_present", True, "profile absent (advisory)")

        # ----- profile_explain_ir_exposed -----
        # The connector always exposes the client-side query IR via .explain().
        ir = client.query(ConfProfileItem).aggregate_count().profile().explain()
        check(
            "profile_explain_ir_exposed",
            isinstance(ir, dict) and ir.get("model") == ConfProfileItem.__name__,
            f"ir_model={ir.get('model') if isinstance(ir, dict) else type(ir).__name__}",
        )

    total = len(passed) + len(failed)
    print(f"\nConnector query-profile conformance: {len(passed)}/{total} passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="AuraDB connector query-profile conformance")
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
