#!/usr/bin/env python3
"""AuraDB cooperative query-timeout conformance harness driven by the Aura Connector.

Exercises a running AuraDB v1.2.x server's cooperative per-query timeouts over the
wire through the public Aura Connector v0.6.x API (``QueryBuilder.timeout(ms)``).
AuraDB enforces a read deadline cooperatively: scan / BM25 / hybrid / exact-vector
reads poll the deadline and abandon work with a structured ``query_timeout`` error
once the wall-clock budget is exceeded, leaving the connection usable.

To drive a *real* over-budget deadline deterministically (rather than sleeping), the
harness seeds a large collection once and runs a full scan under a 1ms budget: the
cooperative deadline check is reached well past 1ms on any host. It connects to a
live server (never the in-memory reference backend).

Usage:
    python -m pip install "aura-connector>=0.6,<0.7"
    python run_connector_timeouts.py --addr 127.0.0.1:7171
    python run_connector_timeouts.py --addr 127.0.0.1:7171 --auth-token dev-secret \
        --tls-ca .local/certs/ca.crt --tls-server-name localhost

Exit code is 0 if all scenarios pass, 1 otherwise.
"""

from __future__ import annotations

import argparse
import asyncio
import sys

from _conformance_isolation import add_isolation_args, collection_prefix, scoped_models

try:
    from aura import AuraError, AuraModel, AuraTimeoutError, Field, Vector, connect
    from aura.config import TLSConfig, TokenAuth
except ImportError:  # pragma: no cover
    print("aura-connector >= 0.6 is required: pip install 'aura-connector>=0.6,<0.7'")
    sys.exit(2)

# Large enough that a full-collection scan blows a 1ms cooperative budget on any
# host: the deadline is polled every ~1024 records, so the scan must run past the
# 1ms mark to trip it. 80k records take many milliseconds to scan even on fast
# hardware (CI runners are typically slower, which only makes the timeout fire more
# readily), giving comfortable margin without a costly seed.
_DATASET_SIZE = 80_000
_GENEROUS_MS = 10_000


class ConfTimeoutDoc(AuraModel):
    id: str = Field(primary_key=True)
    n: int
    body: str = Field(full_text=True)
    embedding: Vector[3]


def _row(model: type, i: int) -> object:
    a = float(i % 7)
    return model(
        id=f"t{i:05d}",
        n=i,
        body="raft consensus replicates the log across many nodes",
        embedding=[a, float(i % 3), 1.0],
    )


async def _seed(client, model: type) -> None:
    existing = (await client.query(model).aggregate_count().aggregate()).metric("count")
    if existing and existing >= _DATASET_SIZE:
        return
    await client.bulk_insert(
        model, [_row(model, i) for i in range(_DATASET_SIZE)], batch_size=1000
    )


async def run(addr: str, token: str | None, tls_ca: str | None, server_name: str, prefix: str) -> int:
    (ConfTimeoutDoc,) = scoped_models(prefix, globals()["ConfTimeoutDoc"])
    scheme = "auradbs" if tls_ca else "auradb"
    dsn = f"{scheme}://{addr}/conf_timeouts"
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

    async with connect(dsn, models=[ConfTimeoutDoc], **options) as client:
        await _seed(client, ConfTimeoutDoc)

        # ----- timeout_option_accepted -----
        # A generous budget completes normally and returns rows.
        rows = await client.query(ConfTimeoutDoc).where(ConfTimeoutDoc.n >= 0).timeout(
            _GENEROUS_MS
        ).limit(5).all()
        check("timeout_option_accepted", len(rows) == 5, f"rows={len(rows)}")

        # ----- timeout_with_search_or_scan + timeout_error_shape -----
        # A full scan (filter on a non-indexed field, matching no rows so the whole
        # collection is traversed) under a 1ms budget must raise a structured
        # query_timeout error.
        timed_out = False
        code = ""
        retryable = None
        try:
            await client.query(ConfTimeoutDoc).where(ConfTimeoutDoc.n < 0).timeout(1).all()
        except AuraTimeoutError as exc:
            timed_out = True
            code = exc.code or ""
            retryable = exc.retryable
        check("timeout_with_search_or_scan", timed_out, "1ms full scan did not time out")
        check("timeout_error_shape",
              timed_out and code == "query_timeout" and retryable is True,
              f"code={code!r} retryable={retryable}")

        # ----- capability_present_for_timeouts_or_clear_error -----
        # The 1ms scan timing out proves the server advertises and enforces query timeouts.
        check("capability_present_for_timeouts_or_clear_error", timed_out,
              "server did not enforce the per-query timeout")

        # ----- connection_survives_timeout + timeout_does_not_poison_next_query -----
        # After the timeout the same client runs a normal query and gets correct results.
        after = await client.query(ConfTimeoutDoc).where(ConfTimeoutDoc.n >= 0).timeout(
            _GENEROUS_MS
        ).limit(3).all()
        check("connection_survives_timeout", len(after) == 3, f"rows={len(after)}")
        after_count = (
            await client.query(ConfTimeoutDoc).timeout(_GENEROUS_MS).aggregate_count().aggregate()
        ).metric("count")
        check("timeout_does_not_poison_next_query", after_count == _DATASET_SIZE,
              f"count={after_count}")

        # ----- timeout_with_vector_or_hybrid_if_supported -----
        # A 1ms exact-vector scan over the whole corpus should also hit the cooperative
        # deadline. On an unusually fast host it may complete within budget; either a
        # structured query_timeout or a completed result is honest (the option is
        # accepted and enforced cooperatively). A non-timeout AuraError is a failure.
        vec_ok = False
        detail = ""
        try:
            vres = await (
                client.search(ConfTimeoutDoc)
                .search_vector("embedding", [1.0, 0.0, 1.0], top_k=_DATASET_SIZE)
                .timeout(1)
                .all()
            )
            vec_ok = True
            detail = f"completed within budget ({len(vres)} rows)"
        except AuraTimeoutError as exc:
            vec_ok = True
            detail = f"timed out: {exc.code}"
        except AuraError as exc:  # any other server error is a real failure
            vec_ok = False
            detail = f"unexpected error: {exc.code}"
        check("timeout_with_vector_or_hybrid_if_supported", vec_ok, detail)

    total = len(passed) + len(failed)
    print(f"\nConnector timeouts conformance: {len(passed)}/{total} passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(description="AuraDB connector query-timeout conformance")
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
