#!/usr/bin/env python3
"""AuraDB search and ranking conformance harness driven by the Aura Connector.

Exercises a running AuraDB v1.1.0 server's BM25 ranked full-text search, exact
vector search, and hybrid retrieval through the public Aura Connector v0.5.0 search
APIs (``search_text`` / ``search_vector`` / ``search_hybrid``), and validates the
typed result scores and capability negotiation.

Usage:
    python -m pip install "aura-connector>=0.5.0"   # or: pip install -e ../aura-connector
    python run_connector_search.py --addr 127.0.0.1:7171
    python run_connector_search.py --addr 127.0.0.1:7171 --auth-token dev-secret \
        --tls-ca .local/certs/ca.crt --tls-server-name localhost

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

from _conformance_isolation import add_isolation_args, collection_prefix, scoped_models

try:
    from aura import AuraModel, Field, Vector, connect, search_scores
    from aura.config import TLSConfig, TokenAuth
except ImportError:  # pragma: no cover
    print("aura-connector >= 0.5.0 is required: pip install 'aura-connector>=0.5.0'")
    sys.exit(2)


class Doc(AuraModel):
    id: str = Field(primary_key=True)
    body: str = Field(full_text=True)
    embedding: Vector[3]


def _doc(model: type, did: str, body: str, emb: list[float]) -> object:
    return model(id=did, body=body, embedding=emb)


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str, prefix: str) -> int:
    (Doc,) = scoped_models(prefix, globals()["Doc"])
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/search"
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

    async with connect(dsn, models=[Doc], **options) as client:
        caps = client.capabilities()
        check("capability_full_text", caps.supports("full_text_search"))
        check("capability_hybrid", caps.supports("hybrid_search"))
        check("capability_vector", caps.supports("vector_search"))

        # Upsert so the harness is idempotent across re-runs against a persistent
        # server (a live cluster keeps its data between conformance runs).
        for d in (
            _doc(Doc, "d1", "raft consensus raft", [1.0, 0.0, 0.0]),
            _doc(Doc, "d2", "the raft module coordinates replicas", [0.0, 1.0, 0.0]),
            _doc(Doc, "d3", "storage compaction and flushing", [0.0, 0.0, 1.0]),
        ):
            await client.upsert(
                Doc, key={"id": d.id}, values={"body": d.body, "embedding": list(d.embedding)}
            )
        check("insert", True)

        bm25 = await client.search(Doc).search_text("body", "raft", rank="bm25").all()
        check("text_search_bm25", len(bm25) == 2 and bm25[0].id == "d1", str([d.id for d in bm25]))
        check("text_search_rank", search_scores(bm25[0]).rank == 1)

        vec = await client.search(Doc).search_vector("embedding", [1.0, 0.0, 0.0], top_k=2).all()
        check("vector_search", len(vec) == 2 and vec[0].id == "d1", str([d.id for d in vec]))

        hyb = await (
            client.search(Doc)
            .search_hybrid("body", "raft", "embedding", [1.0, 0.0, 0.0], top_k=3)
            .all()
        )
        check("hybrid_search", len(hyb) > 0 and search_scores(hyb[0]).score is not None)

    print(f"\nConnector search conformance: {len(passed)}/{len(passed) + len(failed)} passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="AuraDB connector search conformance harness")
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
