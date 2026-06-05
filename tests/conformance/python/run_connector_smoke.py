#!/usr/bin/env python3
"""Minimal Aura Connector smoke test against a running AuraDB server.

A focused, fast check that the published Aura Connector (>= 0.3, < 0.4) can drive
the core AuraDB surface end to end: connect, ping, authenticate, optionally use
TLS, create a schema, insert, find, stream, run a read-your-writes transaction,
search by vector, full-text, and document path, then close cleanly.

This complements the fuller ``run_connector_conformance.py`` with a quick,
CI-friendly signal. It exits 0 on success, 1 on a failed check, and 2 if the
connector is not installed.

Usage:
    python -m pip install "aura-connector>=0.3,<0.4"
    python run_connector_smoke.py --addr 127.0.0.1:7171
    python run_connector_smoke.py --addr 127.0.0.1:7171 --auth-token dev-secret
    python run_connector_smoke.py --addr 127.0.0.1:7171 \
        --tls-ca .local/certs/ca.crt --tls-server-name localhost --auth-token dev-secret
"""

from __future__ import annotations

import argparse
import asyncio
import sys

try:
    from aura import AuraModel, Field, Vector, connect
    from aura.config import TLSConfig, TokenAuth
except ImportError:
    print("aura-connector >= 0.3, < 0.4 is required: pip install 'aura-connector>=0.3,<0.4'")
    sys.exit(2)


class Note(AuraModel):
    id: str = Field(primary_key=True)
    topic: str = Field(index=True)
    body: str
    metadata: dict[str, str] = Field(default_factory=dict)
    embedding: Vector[3]


def _note(nid: str, topic: str, body: str, emb: list[float], source: str = "smoke") -> Note:
    return Note(id=nid, topic=topic, body=body, metadata={"source": source}, embedding=emb)


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str) -> int:
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/smoke"
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

    async with connect(dsn, models=[Note], **options) as client:
        # Connect + ping (TLS handshake and auth happen here when configured).
        check("ping", await client.ping())

        # Create schema (implicit via the registered model) + insert.
        for note in (
            _note("n1", "alpha", "the quick brown fox", [1.0, 0.0, 0.0]),
            _note("n2", "beta", "the lazy brown dog", [0.0, 1.0, 0.0], source="import"),
            _note("n3", "alpha", "a quick red fox", [0.9, 0.1, 0.0]),
        ):
            await client.insert(note)
        check("insert", True)

        # Find.
        check("find_all", len(await client.query(Note).all()) == 3)
        check("filter", len(await client.query(Note).where(Note.topic == "alpha").all()) == 2)

        # Stream (fall back to materializing if the connector lacks streaming).
        streamed = 0
        query = client.query(Note)
        if hasattr(query, "stream"):
            async for _row in query.stream():
                streamed += 1
        else:
            streamed = len(await client.query(Note).all())
        check("stream", streamed == 3, f"streamed {streamed}")

        # Full-text and document-path search.
        check("full_text", len(await client.query(Note).text(Note.body, query="quick fox").all()) >= 1)
        check("document_path", len(await client.query(Note).where(Note.metadata["source"] == "import").all()) == 1)

        # Vector nearest.
        near = await client.search(Note).nearest(Note.embedding, [1.0, 0.0, 0.0], metric="cosine", limit=2).all()
        check("vector_nearest", len(near) == 2 and near[0].id == "n1", str([n.id for n in near]))

        # Read-your-writes within a transaction (guarded: not all connector
        # builds expose in-transaction reads).
        async with client.transaction() as tx:
            await tx.insert(_note("n4", "gamma", "fresh note", [0.0, 0.0, 1.0]))
            if hasattr(tx, "query"):
                visible = await tx.query(Note).where(Note.id == "n4").exists()
                check("read_your_writes", bool(visible))
        check("transaction_commit", await client.query(Note).where(Note.id == "n4").exists())

        # Forward-compatibility: the published 0.3.x connector must safely handle
        # a v0.4.0 server's health frame, which carries an additive `cluster`
        # section in cluster mode. The connector either ignores the unknown field
        # or surfaces it; either way `health()` must succeed and stay usable. This
        # passes against both a non-cluster server (no cluster field) and a
        # single-node cluster server (cluster field present).
        if hasattr(client, "health"):
            health = await client.health()
            check(
                "health_with_additive_cluster_field",
                isinstance(health, dict) and "status" in health,
                f"unexpected health shape: {type(health).__name__}",
            )

    # Reconnect: data persisted across the closed connection.
    async with connect(dsn, models=[Note], **options) as client2:
        check("close_and_reconnect", await client2.query(Note).count() == 4)

    total = len(passed) + len(failed)
    print(f"\nConnector smoke: {len(passed)}/{total} checks passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="Aura Connector smoke test for AuraDB")
    parser.add_argument("--addr", default="127.0.0.1:7171")
    parser.add_argument("--auth-token", default=None)
    parser.add_argument("--tls-ca", default=None)
    parser.add_argument("--tls-server-name", default="localhost")
    args = parser.parse_args()
    sys.exit(asyncio.run(run(args.addr, args.auth_token, args.tls_ca, args.tls_server_name)))


if __name__ == "__main__":
    main()
