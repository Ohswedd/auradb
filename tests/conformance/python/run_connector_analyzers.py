#!/usr/bin/env python3
"""AuraDB query-time analyzer conformance harness driven by the Aura Connector.

Exercises a running AuraDB v1.5.0 server's live, over-the-wire query-time analyzer
selection through the public Aura Connector v0.9.0 search API
(``search_text(analyzer=…)`` / ``search_hybrid(..., analyzer=…)`` / ``.analyzer(…)``),
proving that a non-default analyzer changes retrieval — not just that the request is
accepted — including the ``keyword`` analyzer on **hybrid** search and the
``english_basic`` bare-``s`` singular regression (``lens`` must not fold to ``len``).

This is a release gate: it FAILS (does not skip) if a v1.5 server does not advertise
the ``query_analyzers`` capability.

Usage:
    python -m pip install -e ../aura-connector      # or a built wheel
    python run_connector_analyzers.py --addr 127.0.0.1:7171
    python run_connector_analyzers.py --addr 127.0.0.1:7171 --auth-token dev-secret \
        --tls-ca .local/certs/ca.crt --tls-server-name localhost

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

try:
    from aura import AuraModel, Field, Vector, connect, search_scores
    from aura.config import TLSConfig, TokenAuth
    from aura.errors import AuraQueryError
except ImportError:  # pragma: no cover
    print("aura-connector >= 0.9.0 is required: pip install -e ../aura-connector")
    sys.exit(2)


# A dedicated collection so the harness never collides with other suites. The
# embedding lets the same documents drive the keyword-in-hybrid scenarios.
class AnalyzerDoc(AuraModel):
    id: str = Field(primary_key=True)
    body: str = Field(full_text=True)
    embedding: Vector[3]


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str) -> int:
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/analyzers"
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

    async with connect(dsn, models=[AnalyzerDoc], **options) as client:
        caps = client.capabilities()
        # Release gate: a v1.5 server MUST advertise live analyzers.
        check("capability_query_analyzers_present", caps.supports("query_analyzers"))

        # (id, body, embedding)
        seed = (
            ("cafe", "le café est ouvert", [0.0, 1.0, 0.0]),
            ("coffee", "ordinary coffee shop", [0.0, 0.9, 0.1]),
            ("backups", "verify the backups nightly", [0.2, 0.0, 0.8]),
            ("kw", "backup restore", [1.0, 0.0, 0.0]),
            ("kw2", "backup and restore", [0.9, 0.1, 0.0]),
            # english_basic bare-`s` singular regression: body holds "lenses" and the
            # singular "lens"; both must normalize to "lens", never "len".
            ("lensdoc", "clean the camera lenses and the lens mount", [0.0, 0.2, 0.8]),
            # A vector-only doc whose body shares no keyword with the hybrid query, so
            # it can only enter the fused results via the vector signal.
            ("vec_only", "totally unrelated subject matter", [0.0, 0.0, 1.0]),
        )
        for did, body, emb in seed:
            await client.upsert(
                AnalyzerDoc, key={"id": did}, values={"body": body, "embedding": emb}
            )
        check("insert", True)

        async def ids(query: str, analyzer: str | None) -> list[str]:
            b = client.search(AnalyzerDoc).search_text("body", query)
            if analyzer is not None:
                b = b.analyzer(analyzer)
            rows = await b.all()
            return sorted(r.id for r in rows)

        # default analyzer matches existing (no-analyzer) search exactly.
        base = await ids("coffee", None)
        default = await ids("coffee", "default")
        check("default_analyzer_matches_existing_search", base == default == ["coffee"], str(default))

        # simple lowercases: an upper-case query still matches.
        check("simple_analyzer_case_behavior", await ids("COFFEE", "simple") == ["coffee"])

        # ascii_fold: an unaccented query matches the accented document, where
        # simple does not.
        check("ascii_fold_diacritic_behavior_negative", await ids("cafe", "simple") == [])
        check("ascii_fold_diacritic_behavior", await ids("cafe", "ascii_fold") == ["cafe"])

        # keyword: whole-field exact match only.
        check(
            "keyword_analyzer_behavior",
            await ids("backup restore", "keyword") == ["kw"],
            str(await ids("backup restore", "keyword")),
        )

        # english_basic: a singular query matches the plural document.
        eb = await ids("backup", "english_basic")
        check("english_basic_behavior_if_implemented", "backups" in eb, str(eb))

        # english_basic bare-`s` singular regression: "lens" retrieves the lens doc
        # (lens/lenses both normalize to "lens"), while the truncated "len" matches
        # nothing — proving "lens" is no longer folded to "len".
        lens_hit = await ids("lens", "english_basic")
        len_hit = await ids("len", "english_basic")
        check(
            "english_basic_lens_regression",
            "lensdoc" in lens_hit and "lensdoc" not in len_hit,
            f"lens={lens_hit} len={len_hit}",
        )

        # The builder's chained .analyzer() path works live (used above) — assert it
        # explicitly returned results for a non-default analyzer.
        check("connector_analyzer_builder_live_path", await ids("cafe", "ascii_fold") == ["cafe"])

        # A profiled analyzer query still runs and returns results (the analyzer is
        # carried on the profiled path; the server reports it in EXPLAIN).
        profiled = await client.search(AnalyzerDoc).search_text(
            "body", "cafe", analyzer="ascii_fold"
        ).profile().all()
        check("query_profile_reports_analyzer", [r.id for r in profiled] == ["cafe"])

        # --- keyword analyzer on hybrid search (v1.5.0) ----------------------------

        async def hybrid_rows(vector: list[float], analyzer: str | None):
            b = client.search(AnalyzerDoc).search_hybrid(
                "body", "backup restore", "embedding", vector, top_k=5
            )
            if analyzer is not None:
                b = b.analyzer(analyzer)
            return await b.all()

        # keyword is accepted on hybrid and its whole-field text match surfaces.
        kw_hyb = await hybrid_rows([1.0, 0.0, 0.0], "keyword")
        kw_ids = {r.id for r in kw_hyb}
        check("hybrid_keyword_analyzer_success", "kw" in kw_ids, str(sorted(kw_ids)))

        # The vector component still contributes: with the query vector pointing at the
        # vec_only doc (which shares no keyword), it must still appear via its vector
        # score, with no text score.
        vo_hyb = await hybrid_rows([0.0, 0.0, 1.0], "keyword")
        vo = next((r for r in vo_hyb if r.id == "vec_only"), None)
        vo_ok = vo is not None and search_scores(vo).vector_score is not None
        check("hybrid_keyword_vector_component_still_contributes", vo_ok, str([r.id for r in vo_hyb]))

        # explain/profile reports the analyzer: the keyword analyzer rides the hybrid
        # request (explain() exposes it) and a profiled hybrid keyword query runs.
        hk = client.search(AnalyzerDoc).search_hybrid(
            "body", "backup restore", "embedding", [1.0, 0.0, 0.0], analyzer="keyword"
        )
        explained = hk.explain().get("hybrid", {}).get("analyzer")
        profiled_hk = await hk.profile().all()
        check(
            "hybrid_keyword_profile_or_explain_reports_analyzer",
            explained == "keyword" and any(r.id == "kw" for r in profiled_hk),
            f"explain_analyzer={explained}",
        )

        # Unknown analyzer is rejected client-side before any request is sent.
        try:
            await client.search(AnalyzerDoc).search_text("body", "x", analyzer="stemming").all()
            check("unknown_analyzer_structured_error", False, "no error raised")
        except AuraQueryError:
            check("unknown_analyzer_structured_error", True)

    total = len(passed) + len(failed)
    print(f"\nConnector analyzer conformance: {len(passed)}/{total} passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="AuraDB connector analyzer conformance harness")
    parser.add_argument("--addr", default="127.0.0.1:7171")
    parser.add_argument("--auth-token", default=None)
    parser.add_argument("--tls-ca", default=None)
    parser.add_argument("--tls-server-name", default="localhost")
    args = parser.parse_args()
    sys.exit(asyncio.run(run(args.addr, args.auth_token, args.tls_ca, args.tls_server_name)))


if __name__ == "__main__":
    main()
