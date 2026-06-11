#!/usr/bin/env python3
"""AuraDB facets and aggregations conformance harness driven by the Aura Connector.

Exercises a running AuraDB v1.2.x server's terms facets and count/min/max
aggregations over the wire through the public Aura Connector v0.6.x query API
(``QueryBuilder.facet`` / ``.aggregate_count`` / ``.min`` / ``.max`` / ``.aggregate``),
including BM25 search-scoped facets and deterministic facet tie-breaking.

It connects to a live server (never the in-memory reference backend) and is
idempotent across re-runs: it upserts a fixed dataset under a uniquely named
collection so the counts are stable.

Usage:
    python -m pip install "aura-connector>=0.6,<0.7"   # or: pip install -e ../aura-connector
    python run_connector_facets.py --addr 127.0.0.1:7171
    python run_connector_facets.py --addr 127.0.0.1:7171 --auth-token dev-secret \
        --tls-ca .local/certs/ca.crt --tls-server-name localhost

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

from _conformance_isolation import add_isolation_args, collection_prefix, scoped_models

try:
    from aura import AuraCapabilityError, AuraError, AuraModel, Field, Vector, connect
    from aura.config import TLSConfig, TokenAuth
except ImportError:  # pragma: no cover
    print("aura-connector >= 0.6 is required: pip install 'aura-connector>=0.6,<0.7'")
    sys.exit(2)


class ConfFacetItem(AuraModel):
    id: str = Field(primary_key=True)
    category: str = Field(index=True)
    body: str = Field(full_text=True)
    price: int
    embedding: Vector[3]


# Fixed dataset. Category counts: alpha=3, beta=2, gamma=2 (ties beta), delta=1 (total 8).
# Prices span 10..80 so min/max are unambiguous. "raft" appears in three bodies.
_DATASET = [
    ("i1", "alpha", "raft consensus raft", 10, [1.0, 0.0, 0.0]),
    ("i2", "alpha", "raft module coordinates replicas", 20, [0.9, 0.1, 0.0]),
    ("i3", "alpha", "storage compaction flushing", 30, [0.0, 1.0, 0.0]),
    ("i4", "beta", "raft leader election", 40, [0.8, 0.2, 0.0]),
    ("i5", "beta", "indexing and statistics", 50, [0.0, 0.0, 1.0]),
    ("i6", "gamma", "vector search baseline", 60, [0.1, 0.0, 0.9]),
    ("i7", "gamma", "hybrid retrieval fusion", 70, [0.2, 0.0, 0.8]),
    ("i8", "delta", "backup and restore", 80, [0.0, 0.5, 0.5]),
]


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str, prefix: str) -> int:
    (ConfFacetItem,) = scoped_models(prefix, globals()["ConfFacetItem"])
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/conf_facets"
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

    async with connect(dsn, models=[ConfFacetItem], **options) as client:
        # ----- facets_insert_dataset -----
        for did, cat, body, price, emb in _DATASET:
            await client.upsert(
                ConfFacetItem,
                key={"id": did},
                values={"category": cat, "body": body, "price": price, "embedding": emb},
            )
        check("facets_insert_dataset", True)

        # ----- capability_present_for_facets_or_clear_error -----
        # Against AuraDB v1.2.x the aggregate must run; a backend that cannot serve
        # it raises a structured AuraCapabilityError (the "clear error" branch).
        try:
            base = await client.query(ConfFacetItem).facet("category").aggregate_count().aggregate()
            check("capability_present_for_facets_or_clear_error", base.matched == len(_DATASET),
                  f"matched={base.matched}")
        except AuraCapabilityError as exc:
            # AuraDB v1.2.1 should support facets, so this branch is a genuine failure here.
            check("capability_present_for_facets_or_clear_error", False, f"capability absent: {exc}")
            print(f"\nConnector facets conformance: {len(passed)}/{len(passed) + len(failed)} passed")
            return 1

        # ----- facet_terms_basic -----
        cat_facet = base.facet("category")
        top = cat_facet.buckets[0] if cat_facet and cat_facet.buckets else None
        check("facet_terms_basic", top is not None and top.value == "alpha" and top.count == 3,
              str([(b.value, b.count) for b in (cat_facet.buckets if cat_facet else [])]))

        # ----- facet_terms_limit -----
        limited = await client.query(ConfFacetItem).facet("category", limit=2).aggregate()
        lf = limited.facet("category")
        check("facet_terms_limit", lf is not None and len(lf.buckets) == 2,
              str([(b.value, b.count) for b in (lf.buckets if lf else [])]))

        # ----- facet_terms_tie_break -----
        # beta and gamma both have count 2; deterministic order is count desc, then value asc,
        # so the bucket sequence is alpha, beta, gamma, delta and is stable across runs.
        again = await client.query(ConfFacetItem).facet("category").aggregate()
        seq1 = [(b.value, b.count) for b in cat_facet.buckets] if cat_facet else []
        seq2 = [(b.value, b.count) for b in again.facet("category").buckets]
        expected = [("alpha", 3), ("beta", 2), ("gamma", 2), ("delta", 1)]
        check("facet_terms_tie_break", seq1 == expected and seq2 == expected, f"{seq1} / {seq2}")

        # ----- aggregate_count_all -----
        count_all = await client.query(ConfFacetItem).aggregate_count().aggregate()
        check("aggregate_count_all", count_all.metric("count") == len(_DATASET),
              str(count_all.metric("count")))

        # ----- aggregate_count_filtered -----
        filtered = (
            await client.query(ConfFacetItem)
            .where(ConfFacetItem.price >= 40)
            .aggregate_count()
            .aggregate()
        )
        # prices 40,50,60,70,80 -> 5 rows
        check("aggregate_count_filtered",
              filtered.metric("count") == 5 and filtered.filter_present,
              f"count={filtered.metric('count')} filter_present={filtered.filter_present}")

        # ----- aggregate_min_max_numeric -----
        mm = await client.query(ConfFacetItem).min("price").max("price").aggregate()
        check("aggregate_min_max_numeric",
              mm.metric("min", "price") == 10 and mm.metric("max", "price") == 80,
              f"min={mm.metric('min', 'price')} max={mm.metric('max', 'price')}")

        # ----- search_bm25_with_facets_if_supported -----
        # Scope a facet to the BM25 candidate set for "raft" (i1,i2,i4 -> alpha x2, beta x1).
        scoped = (
            await client.query(ConfFacetItem)
            .search_text("body", "raft")
            .facet("category")
            .aggregate()
        )
        sf = scoped.facet("category")
        scoped_counts = {b.value: b.count for b in sf.buckets} if sf else {}
        check("search_bm25_with_facets_if_supported",
              scoped.search_scoped and scoped_counts.get("alpha") == 2
              and scoped_counts.get("beta") == 1,
              f"search_scoped={scoped.search_scoped} counts={scoped_counts}")

        # ----- hybrid_search_with_facets_if_supported -----
        # The server scopes aggregate facets by a BM25 text_search clause; a hybrid-scoped
        # aggregate either executes (returning a valid result) or is rejected with a
        # structured error. Both are honest; a silent wrong-answer is not.
        try:
            hyb = await (
                client.query(ConfFacetItem)
                .search_hybrid("body", "raft", "embedding", [1.0, 0.0, 0.0], top_k=5)
                .facet("category")
                .aggregate()
            )
            check("hybrid_search_with_facets_if_supported",
                  hyb.facet("category") is not None,
                  "hybrid-scoped aggregate returned no facet")
        except AuraError as exc:
            check("hybrid_search_with_facets_if_supported", True,
                  f"rejected with structured error: {exc.code}")

        # ----- facet_explain_or_explain_analyze_if_connector_exposes_it -----
        # The connector exposes the client-side Query IR via .explain() (a dict over the
        # AuraDB collection), and the requested facet is recorded on the builder AST. The
        # server, in turn, reports per-facet index use (used_index) on the aggregate result
        # — the honest "did this facet use an index" plan signal.
        agg_builder = client.query(ConfFacetItem).facet("category").aggregate_count()
        ir = agg_builder.explain()

        def _facet_field(f: object) -> object:
            return f.get("field") if isinstance(f, dict) else getattr(f, "field", None)

        facet_in_ast = any(_facet_field(f) == "category"
                           for f in getattr(agg_builder.query, "facets", ()))
        used_index_present = cat_facet is not None and isinstance(cat_facet.used_index, bool)
        check("facet_explain_or_explain_analyze_if_connector_exposes_it",
              isinstance(ir, dict) and ir.get("model") == ConfFacetItem.__name__
              and facet_in_ast and used_index_present,
              f"ir_model={ir.get('model')} facet_in_ast={facet_in_ast} "
              f"used_index_present={used_index_present}")

    total = len(passed) + len(failed)
    print(f"\nConnector facets conformance: {len(passed)}/{total} passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="AuraDB connector facets/aggregations conformance")
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
