#!/usr/bin/env python3
"""AuraDB search snippet/highlight conformance harness driven by the Aura Connector.

Exercises a running AuraDB v1.5.0 server's live, opt-in search snippets through the
public Aura Connector v0.9.0 API (``QueryBuilder.snippets(...)`` +
``aura.search_snippets(row)``), proving snippets are produced, field-allowlisted,
capped, Unicode-safe, and never leak an unrequested field.

This is a release gate: it FAILS (does not skip) if a v1.5 server does not advertise
the ``search_snippets`` capability.

Usage:
    python -m pip install -e ../aura-connector      # or a built wheel
    python run_connector_snippets.py --addr 127.0.0.1:7171

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

from _conformance_isolation import add_isolation_args, collection_prefix, scoped_models

try:
    from aura import AuraModel, Field, connect, search_snippets
    from aura.config import TLSConfig, TokenAuth
except ImportError:  # pragma: no cover
    print("aura-connector >= 0.9.0 is required: pip install -e ../aura-connector")
    sys.exit(2)


class SnippetDoc(AuraModel):
    id: str = Field(primary_key=True)
    body: str = Field(full_text=True)
    secret: str


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str, prefix: str) -> int:
    (SnippetDoc,) = scoped_models(prefix, globals()["SnippetDoc"])
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/snippets"
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

    async with connect(dsn, models=[SnippetDoc], **options) as client:
        caps = client.capabilities()
        check("capability_search_snippets_present", caps.supports("search_snippets"))

        await client.upsert(
            SnippetDoc,
            key={"id": "d1"},
            values={
                "body": "create, verify, and restore an AuraDB backup; then verify the backup",
                "secret": "restore TOP-SECRET token",
            },
        )
        await client.upsert(
            SnippetDoc,
            key={"id": "d2"},
            values={"body": "le café est ouvert tard", "secret": "n/a"},
        )
        check("insert", True)

        # snippet_opt_in_only: a search WITHOUT a snippet request yields no snippets.
        rows = await client.search(SnippetDoc).search_text("body", "restore").all()
        check(
            "snippet_opt_in_only",
            all(len(search_snippets(r)) == 0 for r in rows),
        )

        # snippet_basic_fragment: requesting body snippets yields a highlighted fragment.
        rows = await (
            client.search(SnippetDoc)
            .search_text("body", "restore")
            .snippets(fields=["body"])
            .all()
        )
        d1 = next((r for r in rows if r.id == "d1"), None)
        snips = search_snippets(d1) if d1 is not None else ()
        body_snips = [s for s in snips if s.field == "body"]
        ok_basic = bool(body_snips) and any(f.ranges for f in body_snips[0].fragments)
        check("snippet_basic_fragment", ok_basic, str(snips))
        if ok_basic:
            frag = body_snips[0].fragments[0]
            r = frag.ranges[0]
            check("snippet_ranges_match_text", frag.text[r.start : r.end].lower() == "restore")

        # snippet_field_allowlist / no_hidden_fields: only "body" is returned; the
        # "secret" field (which also contains "restore") never appears.
        check("snippet_field_allowlist", all(s.field == "body" for s in snips))
        joined = " ".join(f.text for s in snips for f in s.fragments)
        check("snippet_no_hidden_fields", "TOP-SECRET" not in joined and "token" not in joined)

        # snippet_fragment_caps: a tight cap is honored.
        rows = await (
            client.search(SnippetDoc)
            .search_text("body", "verify")
            .snippets(fields=["body"], max_fragments=1, fragment_chars=15)
            .all()
        )
        capped = [s for r in rows for s in search_snippets(r) if s.field == "body"]
        ok_caps = all(
            len(s.fragments) <= 1 and all(len(f.text) <= 15 for f in s.fragments) for s in capped
        )
        check("snippet_fragment_caps", ok_caps and bool(capped))

        # snippet_unicode_safe: an ascii_fold query against an accented field slices
        # exactly the accented word on a char boundary.
        rows = await (
            client.search(SnippetDoc)
            .search_text("body", "cafe", analyzer="ascii_fold")
            .snippets(fields=["body"])
            .all()
        )
        d2 = next((r for r in rows if r.id == "d2"), None)
        usnips = [s for s in (search_snippets(d2) if d2 else ()) if s.field == "body"]
        ok_uni = bool(usnips) and any(
            frag.text[r.start : r.end] == "café" for frag in usnips[0].fragments for r in frag.ranges
        )
        check("snippet_unicode_safe", ok_uni, str(usnips))

        # snippet_missing_field_safe: naming a non-existent field yields no snippet,
        # no crash, and the valid field still works.
        rows = await (
            client.search(SnippetDoc)
            .search_text("body", "restore")
            .snippets(fields=["does_not_exist", "body"])
            .all()
        )
        safe = [s for r in rows for s in search_snippets(r)]
        check("snippet_missing_field_safe", all(s.field == "body" for s in safe))

        # connector_snippet_models_live_path: the typed models came back populated.
        check(
            "connector_snippet_models_live_path",
            ok_basic and body_snips[0].fragments[0].text != "",
        )

    total = len(passed) + len(failed)
    print(f"\nConnector snippet conformance: {len(passed)}/{total} passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="AuraDB connector snippet conformance harness")
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
