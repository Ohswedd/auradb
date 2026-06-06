#!/usr/bin/env python3
"""Aura Connector cluster conformance against an AuraDB multi-node preview.

Drives the connector's cluster ergonomics (Aura Connector >= 0.4) against a live
preview cluster: a write to the leader succeeds, a write to a follower raises
``AuraNotLeaderError`` carrying the leader address, the manual reconnect helper
reaches the leader, the bounded redirect helper applies the write exactly once
without an unbounded retry loop, and a transaction is never auto-redirected.
Authentication and TLS, when configured, are preserved across a redirect.

AuraDB multi-node mode is experimental and opt-in; this is preview conformance,
not a production high-availability claim.

Exit codes: 0 success, 1 a failed check, 2 the connector is missing/too old.

Usage:
    python -m pip install "aura-connector>=0.4,<0.5"
    python run_connector_cluster.py --leader 127.0.0.1:7171 --follower 127.0.0.1:7172
    python run_connector_cluster.py --leader L --follower F \
        --auth-token dev-secret --tls-ca .local/certs/ca.crt
"""

from __future__ import annotations

import argparse
import asyncio
import sys

try:
    from aura import AuraModel, AuraNotLeaderError, Field, connect
    from aura.config import TLSConfig, TokenAuth
    from aura.errors import AuraTransactionError
except ImportError:
    print("aura-connector >= 0.4, < 0.5 is required: pip install 'aura-connector>=0.4,<0.5'")
    sys.exit(2)


class CItem(AuraModel):
    id: int = Field(primary_key=True)
    label: str


def _dsn(addr: str, tls_ca: str | None) -> str:
    return f"{'auradbs' if tls_ca else 'auradb'}://{addr}/cluster"


def _options(token: str | None, tls_ca: str | None) -> dict:
    options: dict = {}
    if token:
        options["auth"] = TokenAuth(token)
    if tls_ca:
        options["tls"] = TLSConfig(enabled=True, ca_cert_path=tls_ca, verify_hostname=True)
    return options


async def run(leader: str, follower: str, token: str | None, tls_ca: str | None) -> int:
    leader_dsn = _dsn(leader, tls_ca)
    follower_dsn = _dsn(follower, tls_ca)
    options = _options(token, tls_ca)

    passed: list[str] = []
    failed: list[str] = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        (passed if ok else failed).append(name)
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}" + ("" if ok else f": {detail}"))

    # 1. Connect to the leader and write to it.
    async with connect(leader_dsn, models=[CItem], **options) as client:
        check("connect_leader", await client.ping())
        await client.upsert(CItem, key={"id": 1}, values={"label": "leader-write"})
        check("write_leader", (await client.CItem.find(id=1)).label == "leader-write")

    # 2-4. Connect to a follower; a write raises AuraNotLeaderError exposing the
    # leader address; the reconnect helper reaches the leader and applies the write.
    async with connect(follower_dsn, models=[CItem], **options) as client:
        try:
            await client.insert(CItem(id=2, label="rejected"))
            check("follower_not_leader", False, "follower accepted a write")
        except AuraNotLeaderError as exc:
            check("follower_not_leader", True)
            check("error_exposes_leader_addr", bool(exc.leader_addr), repr(exc.leader_addr))
            check("not_leader_retryable", exc.retryable is True)
            leader_client = await client.connect_to_leader(exc)
            try:
                await leader_client.upsert(CItem, key={"id": 2}, values={"label": "via-reconnect"})
                check(
                    "reconnect_helper_writes_to_leader",
                    (await leader_client.CItem.find(id=2)).label == "via-reconnect",
                )
                # Auth/TLS preserved across the redirect: the new client carries the
                # same credential object and TLS settings.
                check(
                    "auth_tls_preserved",
                    leader_client.config.auth is client.config.auth
                    and leader_client.config.tls == client.config.tls,
                )
            finally:
                await leader_client.close()

    # 5. The bounded redirect helper resolves the leader and applies the write once,
    # without an unbounded retry loop.
    async with connect(follower_dsn, models=[CItem], **options) as client:
        redirect = client.with_leader_redirect(max_redirects=1)
        await redirect.upsert(CItem, key={"id": 3}, values={"label": "redirected"})
        check("redirect_helper_bounded", (await client.CItem.find(id=3)).label == "redirected")

    # 6. A transaction is never auto-redirected, and the helper refuses to wrap one.
    async with connect(follower_dsn, models=[CItem], **options) as client:
        try:
            async with client.transaction() as txn:
                await txn.insert(CItem(id=4, label="in-txn"))
            check("transaction_not_redirected", False, "transaction unexpectedly succeeded")
        except AuraNotLeaderError:
            check("transaction_not_redirected", True)
        try:
            client.with_leader_redirect().transaction()
            check("redirect_rejects_transaction", False, "helper wrapped a transaction")
        except AuraTransactionError:
            check("redirect_rejects_transaction", True)

    total = len(passed) + len(failed)
    print(f"\nConnector cluster conformance: {len(passed)}/{total} checks passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="Aura Connector cluster conformance for AuraDB")
    parser.add_argument("--leader", required=True, help="leader client address host:port")
    parser.add_argument("--follower", required=True, help="follower client address host:port")
    parser.add_argument("--auth-token", default=None)
    parser.add_argument("--tls-ca", default=None)
    args = parser.parse_args()
    sys.exit(asyncio.run(run(args.leader, args.follower, args.auth_token, args.tls_ca)))


if __name__ == "__main__":
    main()
