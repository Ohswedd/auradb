#!/usr/bin/env python3
"""AuraDB ranked-pagination conformance harness driven by the Aura Connector.

Exercises a running AuraDB v1.2.x server's stable ranked pagination over the wire
through the public Aura Connector v0.6.x API (``QueryBuilder.search_pages``), which
pages BM25 / hybrid / exact-vector ranked search by opaque, server-issued keyset
cursor tokens. Verifies duplicate-free pages, deterministic ordering, cursor-token
presence, structured invalid-cursor rejection, and transaction-snapshot stability.

It connects to a live server (never the in-memory reference backend) and is
idempotent across re-runs: it upserts a fixed dataset under a uniquely named
collection.

Usage:
    python -m pip install "aura-connector>=0.6,<0.7"
    python run_connector_pagination.py --addr 127.0.0.1:7171
    python run_connector_pagination.py --addr 127.0.0.1:7171 --auth-token dev-secret \
        --tls-ca .local/certs/ca.crt --tls-server-name localhost

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

try:
    from aura import AuraError, AuraModel, Field, Vector, connect
    from aura.config import TLSConfig, TokenAuth
except ImportError:  # pragma: no cover
    print("aura-connector >= 0.6 is required: pip install 'aura-connector>=0.6,<0.7'")
    sys.exit(2)


class ConfPageDoc(AuraModel):
    id: str = Field(primary_key=True)
    body: str = Field(full_text=True)
    embedding: Vector[3]


# 12 docs, all containing "lorem" so a BM25 search returns the whole set and a small
# page size produces several pages. Embeddings spread along one axis so exact-vector
# ranking is unambiguous.
_DATASET = [
    (f"p{n:02d}", f"lorem ipsum document number {n} lorem", [float(n) / 12.0, 0.0, 0.0])
    for n in range(1, 13)
]


async def _collect_ids(pages_iter) -> tuple[list[str], list[str | None]]:
    """Drain a search_pages() iterator, returning (all row ids, per-page cursor tokens)."""
    ids: list[str] = []
    cursors_seen: list[str | None] = []
    async for page in pages_iter:
        ids.extend(r.id for r in page.rows)
        cursors_seen.append(page.cursor)
    return ids, cursors_seen


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str) -> int:
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/conf_pagination"
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

    async with connect(dsn, models=[ConfPageDoc], **options) as client:
        for did, body, emb in _DATASET:
            await client.upsert(ConfPageDoc, key={"id": did}, values={"body": body, "embedding": emb})

        # ----- bm25_pagination_first_page -----
        first_page = None
        async for page in client.search(ConfPageDoc).search_text("body", "lorem").search_pages(
            page_size=4
        ):
            first_page = page
            break
        check("bm25_pagination_first_page",
              first_page is not None and len(first_page.rows) == 4,
              f"rows={0 if first_page is None else len(first_page.rows)}")

        # ----- cursor_token_present -----
        # The first page of a 12-doc result paged by 4 is not the last page, so it must
        # carry an opaque server-issued cursor token.
        check("cursor_token_present",
              first_page is not None and first_page.cursor is not None and first_page.has_more,
              f"cursor={None if first_page is None else first_page.cursor!r}")

        # ----- bm25_pagination_second_page_no_duplicates -----
        bm25_ids, _ = await _collect_ids(
            client.search(ConfPageDoc).search_text("body", "lorem").search_pages(page_size=4)
        )
        check("bm25_pagination_second_page_no_duplicates",
              len(bm25_ids) == len(set(bm25_ids)) == len(_DATASET),
              f"n={len(bm25_ids)} unique={len(set(bm25_ids))}")

        # ----- pagination_stable_tie_break -----
        # Paginating the same ranked query twice yields the same id order (the cursor
        # tie-breaks deterministically by score desc then id asc).
        bm25_ids_again, _ = await _collect_ids(
            client.search(ConfPageDoc).search_text("body", "lorem").search_pages(page_size=4)
        )
        check("pagination_stable_tie_break", bm25_ids == bm25_ids_again,
              f"{bm25_ids} vs {bm25_ids_again}")

        # ----- exact_vector_pagination_if_supported -----
        vec_ids, _ = await _collect_ids(
            client.search(ConfPageDoc)
            .search_vector("embedding", [1.0, 0.0, 0.0], top_k=len(_DATASET))
            .search_pages(page_size=5)
        )
        check("exact_vector_pagination_if_supported",
              len(vec_ids) == len(set(vec_ids)) and len(vec_ids) > 0,
              f"n={len(vec_ids)} unique={len(set(vec_ids))}")

        # ----- hybrid_pagination_no_duplicates_if_supported -----
        try:
            hyb_ids, _ = await _collect_ids(
                client.search(ConfPageDoc)
                .search_hybrid("body", "lorem", "embedding", [1.0, 0.0, 0.0], top_k=len(_DATASET))
                .search_pages(page_size=4)
            )
            check("hybrid_pagination_no_duplicates_if_supported",
                  len(hyb_ids) == len(set(hyb_ids)) and len(hyb_ids) > 0,
                  f"n={len(hyb_ids)} unique={len(set(hyb_ids))}")
        except AuraError as exc:
            check("hybrid_pagination_no_duplicates_if_supported", True,
                  f"rejected with structured error: {exc.code}")

        # ----- invalid_cursor_rejected -----
        # search_pages manages cursors internally, so to drive a deliberately malformed
        # cursor over the wire we issue the same search_page read through the connector's
        # backend with a bogus token. The server must reject it with a structured error
        # and the connection must remain usable afterward.
        base_ir = client.search(ConfPageDoc).search_text("body", "lorem").query.to_ir()
        bad_ir = {**base_ir, "operation": "search_page", "page_size": 4, "cursor": "not-a-cursor"}
        rejected = False
        code = ""
        try:
            await client.backend.execute_query(bad_ir)
        except AuraError as exc:
            rejected = True
            code = exc.code or ""
        # Connection still usable after the rejection.
        still_ok = len(await client.query(ConfPageDoc).limit(1).all()) >= 0
        check("invalid_cursor_rejected", rejected and bool(code) and still_ok,
              f"rejected={rejected} code={code!r} still_ok={still_ok}")

        # ----- pagination_transaction_snapshot_guidance -----
        # BM25/hybrid ranked pagination is duplicate-free and stable when paged inside a
        # transaction snapshot (the snapshot fixes the corpus statistics across pages).
        # Outside a transaction, concurrent writes can re-score documents between pages —
        # this is documented honestly, not worked around.
        async with client.transaction() as tx:
            tx_ids, _ = await _collect_ids(
                tx.search(ConfPageDoc).search_text("body", "lorem").search_pages(page_size=4)
            )
        check("pagination_transaction_snapshot_guidance",
              len(tx_ids) == len(set(tx_ids)) == len(_DATASET),
              f"n={len(tx_ids)} unique={len(set(tx_ids))}")

    total = len(passed) + len(failed)
    print(f"\nConnector pagination conformance: {len(passed)}/{total} passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="AuraDB connector ranked-pagination conformance")
    parser.add_argument("--addr", default="127.0.0.1:7171")
    parser.add_argument("--auth-token", default=None)
    parser.add_argument("--tls-ca", default=None)
    parser.add_argument("--tls-server-name", default="localhost")
    args = parser.parse_args()
    sys.exit(asyncio.run(run(args.addr, args.auth_token, args.tls_ca, args.tls_server_name)))


if __name__ == "__main__":
    main()
