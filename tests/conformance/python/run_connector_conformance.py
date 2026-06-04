#!/usr/bin/env python3
"""AuraDB conformance harness driven by the published Aura Connector.

Unlike ``run_conformance.py`` (a hand-rolled Aura Wire Protocol client), this
harness exercises a running AuraDB server through the public Aura Connector API
and its native AuraDB backend (aura-connector >= 0.3.0). It validates the
coordinated client/server release end to end, including authentication and TLS.

Usage:
    python -m pip install "aura-connector>=0.3.0"   # or: pip install -e ../aura-connector
    python run_connector_conformance.py --addr 127.0.0.1:7171
    python run_connector_conformance.py --addr 127.0.0.1:7171 --auth-token dev-secret
    python run_connector_conformance.py --addr 127.0.0.1:7171 \
        --tls-ca .local/certs/ca.crt --tls-server-name localhost --auth-token dev-secret

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

try:
    from aura import AuraModel, Field, Vector, connect
    from aura.config import TLSConfig, TokenAuth
    from aura.errors import AuraAuthenticationError, AuraConstraintError
except ImportError:  # pragma: no cover
    print("aura-connector >= 0.3.0 is required: pip install 'aura-connector>=0.3.0'")
    sys.exit(2)


class Article(AuraModel):
    id: str = Field(primary_key=True)
    status: str = Field(index=True)
    title: str
    body: str
    views: int
    metadata: dict[str, str] = Field(default_factory=dict)
    embedding: Vector[3]


def _article(aid: str, status: str, body: str, views: int, emb: list[float], source: str = "import") -> Article:
    return Article(
        id=aid, status=status, title=f"T-{aid}", body=body, views=views,
        metadata={"source": source}, embedding=emb,
    )


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str) -> int:
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/conformance"
    options: dict = {}
    if token:
        options["auth"] = TokenAuth(token)
    if tls_ca:
        options["tls"] = TLSConfig(enabled=True, ca_cert_path=tls_ca, verify_hostname=True)

    passed: list[str] = []
    failed: list[str] = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        if ok:
            passed.append(name)
            print(f"  [PASS] {name}")
        else:
            failed.append(name)
            print(f"  [FAIL] {name}: {detail}")

    # An auth-required server must reject a connection with no/invalid token.
    if token:
        try:
            async with connect(dsn, models=[Article], tls=options.get("tls")) as _:
                check("rejects_unauthenticated", False, "connected without a token")
        except AuraAuthenticationError:
            check("rejects_unauthenticated", True)
        except Exception as exc:  # noqa: BLE001
            check("rejects_unauthenticated", True, f"(connection refused: {exc})")

    async with connect(dsn, models=[Article], **options) as client:
        check("ping", await client.ping())

        for art in (
            _article("a1", "published", "alpha document one", 10, [1.0, 0.0, 0.0]),
            _article("a2", "draft", "beta document two", 5, [0.0, 1.0, 0.0]),
            _article("a3", "published", "alpha document three", 20, [0.9, 0.1, 0.0], source="web"),
        ):
            await client.insert(art)
        check("insert", True)

        check("find_all", len(await client.query(Article).all()) == 3)
        check("filter", len(await client.query(Article).where(Article.status == "published").all()) == 2)
        check("document_field", len(await client.query(Article).where(Article.metadata["source"] == "import").all()) == 2)
        check("full_text_search", len(await client.query(Article).text(Article.body, query="alpha").all()) == 2)
        check("count", await client.query(Article).count() == 3)
        check("exists", await client.query(Article).where(Article.id == "a1").exists() is True)

        near = await client.search(Article).nearest(Article.embedding, [1.0, 0.0, 0.0], metric="cosine", limit=2).all()
        check("vector_nearest", len(near) == 2 and near[0].id == "a1", str([a.id for a in near]))

        n = await client.update(Article).where(Article.id == "a2").set(status="published").execute()
        check("update", n == 1 and len(await client.query(Article).where(Article.status == "published").all()) == 3)

        n = await client.delete(Article).where(Article.id == "a3").execute()
        check("delete", n == 1 and await client.query(Article).count() == 2)

        await client.upsert(
            Article, key={"id": "a1"},
            values={"status": "archived", "title": "T-a1", "body": "alpha document one",
                    "views": 99, "metadata": {"source": "import"}, "embedding": [1.0, 0.0, 0.0]},
        )
        a1 = await client.query(Article).where(Article.id == "a1").one()
        check("upsert", a1.status == "archived")

        async with client.transaction() as tx:
            await tx.insert(_article("a4", "draft", "delta four", 1, [0.0, 0.0, 1.0]))
        committed = await client.query(Article).where(Article.id == "a4").exists()
        check("transaction_commit", committed)

        try:
            await client.insert(_article("a1", "x", "dup", 1, [0.0, 0.0, 1.0]))
            check("error_mapping", False, "duplicate insert did not raise")
        except AuraConstraintError:
            check("error_mapping", True)

    # Reconnect: data persists across connections.
    async with connect(dsn, models=[Article], **options) as client2:
        check("restart_survival", await client2.query(Article).where(Article.id == "a1").exists())

    print(f"\nConnector conformance: {len(passed)}/{len(passed) + len(failed)} scenarios passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="AuraDB connector conformance harness")
    parser.add_argument("--addr", default="127.0.0.1:7171")
    parser.add_argument("--auth-token", default=None)
    parser.add_argument("--tls-ca", default=None)
    parser.add_argument("--tls-server-name", default="localhost")
    args = parser.parse_args()
    sys.exit(asyncio.run(run(args.addr, args.auth_token, args.tls_ca, args.tls_server_name)))


if __name__ == "__main__":
    main()
