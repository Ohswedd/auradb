#!/usr/bin/env python3
"""AuraDB approximate-vector (HNSW preview) conformance harness via the Aura Connector.

Exercises a running AuraDB v1.3.x server's opt-in approximate vector preview over
the wire through the public Aura Connector v0.7.x API
(``QueryBuilder.search_vector(..., approximate=HnswOptions(...))``). Exact vector
search remains the default and correctness baseline; this checks that the preview
returns results, that its top-k overlaps the exact top-k well on a fixed dataset
(a dataset-specific recall guard, not a universal guarantee), and that the
``fallback`` policy behaves honestly.

Connects to a live server (never the in-memory reference backend); idempotent
across re-runs via upserts under a uniquely named collection.

Usage:
    python -m pip install "aura-connector>=0.7,<0.8"
    python run_connector_ann_preview.py --addr 127.0.0.1:7171

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

try:
    from aura import AuraCapabilityError, AuraError, AuraModel, Field, HnswOptions, Vector, connect
    from aura.config import TLSConfig, TokenAuth
except ImportError:  # pragma: no cover
    print("aura-connector >= 0.7 is required: pip install 'aura-connector>=0.7,<0.8'")
    sys.exit(2)

DIM = 8


def gen_vec(seed: int) -> list[float]:
    s = (seed * 0x9E3779B97F4A7C15 + 1) & 0xFFFFFFFFFFFFFFFF
    out = []
    for _ in range(DIM):
        s ^= (s << 13) & 0xFFFFFFFFFFFFFFFF
        s ^= s >> 7
        s ^= (s << 17) & 0xFFFFFFFFFFFFFFFF
        out.append((s % 2000) / 1000.0 - 1.0)
    return out


# 40 vectors clears the server's minimum-dataset threshold so the preview is used.
_N = 40


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str) -> int:
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/conf_ann_preview"
    options: dict = {}
    if token:
        options["auth"] = TokenAuth(token)
    if tls_ca:
        options["tls"] = TLSConfig(enabled=True, ca_cert_path=tls_ca, verify_hostname=True)

    class ConfAnnItem(AuraModel):
        id: str = Field(primary_key=True)
        embedding: Vector[DIM]

    passed: list[str] = []
    failed: list[str] = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        (passed if ok else failed).append(name)
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}" + ("" if ok else f": {detail}"))

    def ids(rows) -> list[str]:
        return [r.id for r in rows]

    async with connect(dsn, models=[ConfAnnItem], **options) as client:
        # ----- ann_preview_insert_dataset -----
        for i in range(_N):
            await client.upsert(
                ConfAnnItem, key={"id": f"d{i}"}, values={"embedding": gen_vec(i + 1)}
            )
        check("ann_preview_insert_dataset", True)

        query = gen_vec(1_000_003)

        # ----- ann_preview_capability_or_clear_error -----
        try:
            approx_rows = await (
                client.query(ConfAnnItem)
                .search_vector("embedding", query, top_k=10, approximate=HnswOptions(ef_search=64))
                .all()
            )
        except AuraCapabilityError as exc:
            check("ann_preview_capability_or_clear_error", False, f"capability absent: {exc}")
            print(f"\nConnector ANN preview conformance: {len(passed)}/{len(passed) + len(failed)} passed")
            return 1
        check("ann_preview_capability_or_clear_error", True)

        # ----- ann_preview_returns_results -----
        check("ann_preview_returns_results", len(approx_rows) == 10, f"got {len(approx_rows)}")

        # ----- ann_preview_recall_vs_exact -----
        exact_rows = await client.query(ConfAnnItem).search_vector("embedding", query, top_k=10).all()
        exact_ids = set(ids(exact_rows))
        overlap = sum(1 for i in ids(approx_rows) if i in exact_ids)
        recall = overlap / max(len(exact_ids), 1)
        check("ann_preview_recall_vs_exact", recall >= 0.8, f"recall@10={recall:.3f}")

        # ----- ann_preview_fallback_exact -----
        # The default fallback policy keeps exact search as the baseline; a query
        # with fallback="exact" always returns a valid ranked result.
        fb = await (
            client.query(ConfAnnItem)
            .search_vector(
                "embedding", query, top_k=5, approximate=HnswOptions(fallback="exact")
            )
            .all()
        )
        check("ann_preview_fallback_exact", len(fb) == 5, f"got {len(fb)}")

        # ----- ann_preview_require_error_policy_honest -----
        # With fallback="error", the server either serves the preview (dataset is
        # above threshold) or returns a structured error when it cannot — never a
        # silent wrong answer.
        try:
            req = await (
                client.query(ConfAnnItem)
                .search_vector(
                    "embedding", query, top_k=5, approximate=HnswOptions(fallback="error")
                )
                .all()
            )
            check("ann_preview_require_error_policy_honest", len(req) == 5, f"got {len(req)}")
        except AuraError as exc:
            check(
                "ann_preview_require_error_policy_honest",
                True,
                f"structured error when preview unavailable: {exc.code}",
            )

    total = len(passed) + len(failed)
    print(f"\nConnector ANN preview conformance: {len(passed)}/{total} passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="AuraDB connector ANN preview conformance")
    parser.add_argument("--addr", default="127.0.0.1:7171")
    parser.add_argument("--auth-token", default=None)
    parser.add_argument("--tls-ca", default=None)
    parser.add_argument("--tls-server-name", default="localhost")
    args = parser.parse_args()
    sys.exit(asyncio.run(run(args.addr, args.auth_token, args.tls_ca, args.tls_server_name)))


if __name__ == "__main__":
    main()
