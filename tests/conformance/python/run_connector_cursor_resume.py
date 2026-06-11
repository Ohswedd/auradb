#!/usr/bin/env python3
"""AuraDB public ranked-cursor resume conformance harness via the Aura Connector.

Exercises a running AuraDB v1.3.x server's ranked-search pagination resumed from
an externally-held opaque cursor token through the public Aura Connector v0.7.x
API (``QueryBuilder.page(page_size=...)`` plus ``Client.resume_search``). The
token is opaque and forwarded verbatim; pages must not overlap, and an invalid
token must be rejected with a structured error.

Connects to a live server (never the in-memory reference backend); idempotent
across re-runs via upserts under a uniquely named collection.

Usage:
    python -m pip install "aura-connector>=0.7,<0.8"
    python run_connector_cursor_resume.py --addr 127.0.0.1:7171

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

from _conformance_isolation import add_isolation_args, collection_prefix, scoped_models

try:
    from aura import AuraError, AuraModel, Field, connect
    from aura.config import TLSConfig, TokenAuth
except ImportError:  # pragma: no cover
    print("aura-connector >= 0.7 is required: pip install 'aura-connector>=0.7,<0.8'")
    sys.exit(2)


class ConfCursorDoc(AuraModel):
    id: str = Field(primary_key=True)
    body: str = Field(full_text=True)


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str, prefix: str) -> int:
    (ConfCursorDoc,) = scoped_models(prefix, globals()["ConfCursorDoc"])
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/conf_cursor_resume"
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

    async with connect(dsn, models=[ConfCursorDoc], **options) as client:
        # ----- cursor_insert_dataset (12 docs all matching "alpha") -----
        for i in range(12):
            await client.upsert(
                ConfCursorDoc, key={"id": f"d{i}"}, values={"body": f"alpha beta gamma {i}"}
            )
        check("cursor_insert_dataset", True)

        search = client.search(ConfCursorDoc).search_text("body", "alpha")

        # ----- cursor_first_page -----
        first = await search.page(page_size=4)
        first_ids = [r.id for r in first.items]
        check(
            "cursor_first_page",
            len(first.items) == 4 and first.has_more and first.next_cursor is not None,
            f"ids={first_ids} has_more={first.has_more}",
        )

        # ----- cursor_token_opaque -----
        check(
            "cursor_token_opaque",
            isinstance(first.next_cursor, str) and len(first.next_cursor) > 0,
            f"cursor type={type(first.next_cursor).__name__}",
        )

        # ----- cursor_resume_external_token (no overlap across pages) -----
        seen = set(first_ids)
        token_cur = first.next_cursor
        pages = 1
        overlap = False
        while token_cur is not None and pages < 10:
            page = await client.resume_search(search, token_cur, page_size=4)
            page_ids = [r.id for r in page.items]
            if any(i in seen for i in page_ids):
                overlap = True
            seen.update(page_ids)
            token_cur = page.next_cursor
            pages += 1
        check(
            "cursor_resume_external_token",
            not overlap and len(seen) == 12,
            f"distinct={len(seen)} overlap={overlap} pages={pages}",
        )

        # ----- cursor_invalid_token_rejected -----
        try:
            await client.resume_search(search, "not-a-valid-cursor-token", page_size=4)
            check("cursor_invalid_token_rejected", False, "invalid token was accepted")
        except AuraError as exc:
            check("cursor_invalid_token_rejected", True, f"rejected: {exc.code}")

    total = len(passed) + len(failed)
    print(f"\nConnector cursor-resume conformance: {len(passed)}/{total} passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="AuraDB connector cursor-resume conformance")
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
