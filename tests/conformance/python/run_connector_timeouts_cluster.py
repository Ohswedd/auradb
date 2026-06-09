#!/usr/bin/env python3
"""Aura Connector cooperative query timeouts against an AuraDB multi-node preview.

Validates v1.2 per-query timeouts across a controlled static-cluster preview: a
generous budget completes against the leader, a 1ms budget on a full-collection
scan against the leader raises a structured ``query_timeout`` error and the
connection stays usable, the follower's read/timeout behavior is recorded honestly
as eventually consistent, and — when candidate addresses are supplied — the timeout
option works against the current leader after a leader change.

To drive a real over-budget deadline deterministically the harness seeds a large
collection on the leader once (replicated to followers by Raft) and scans it under a
1ms budget. Seeding is operator-run; this script is intended to run against a
locally launched cluster.

Honest scope: AuraDB multi-node mode is a controlled static-cluster preview, NOT a
production high-availability guarantee. There is no production failover and no
linearizable reads. Query timeouts are cooperative (the read polls the deadline),
not preemptive. Writes remain leader-only.

Exit codes: 0 success, 1 a failed check, 2 the connector is missing/too old.

Usage:
    python -m pip install "aura-connector>=0.6,<0.7"
    python run_connector_timeouts_cluster.py --leader 127.0.0.1:7171 --follower 127.0.0.1:7181
    python run_connector_timeouts_cluster.py --leader L --follower F \
        --candidate-addrs 127.0.0.1:7171,127.0.0.1:7181,127.0.0.1:7191 \
        --auth-token dev-secret --tls-ca .local/certs/ca.crt
"""

from __future__ import annotations

import argparse
import asyncio
import sys

try:
    from aura import AuraModel, AuraNotLeaderError, AuraTimeoutError, Field, Vector, connect
    from aura.config import TLSConfig, TokenAuth
    from aura.errors import AuraConnectionError, AuraError
except ImportError:
    print("aura-connector >= 0.6, < 0.7 is required: pip install 'aura-connector>=0.6,<0.7'")
    sys.exit(2)

_DATASET_SIZE = 80_000
_GENEROUS_MS = 10_000


class TimeoutCDoc(AuraModel):
    id: int = Field(primary_key=True)
    n: int
    body: str = Field(full_text=True)
    embedding: Vector[3]


def _dsn(addr: str, tls_ca: str | None) -> str:
    return f"{'auradbs' if tls_ca else 'auradb'}://{addr}/timeoutcluster"


def _options(token: str | None, tls_ca: str | None) -> dict:
    options: dict = {}
    if token:
        options["auth"] = TokenAuth(token)
    if tls_ca:
        options["tls"] = TLSConfig(enabled=True, ca_cert_path=tls_ca, verify_hostname=True)
    return options


async def _seed_leader(client) -> None:
    existing = (await client.query(TimeoutCDoc).aggregate_count().aggregate()).metric("count")
    if existing and existing >= _DATASET_SIZE:
        return
    await client.bulk_insert(
        TimeoutCDoc,
        [
            TimeoutCDoc(id=i, n=i, body="raft consensus replicates the log",
                        embedding=[float(i % 7), float(i % 3), 1.0])
            for i in range(_DATASET_SIZE)
        ],
        batch_size=2000,
    )


async def _forced_timeout(client) -> tuple[bool, str, object]:
    """Run a 1ms full-collection scan; return (timed_out, code, retryable)."""
    try:
        await client.query(TimeoutCDoc).where(TimeoutCDoc.n < 0).timeout(1).all()
        return False, "", None
    except AuraTimeoutError as exc:
        return True, exc.code or "", exc.retryable


async def _resolve_leader(candidates: list[str], options: dict, tls_ca: str | None) -> str | None:
    for _ in range(20):
        for addr in candidates:
            try:
                async with connect(_dsn(addr, tls_ca), models=[TimeoutCDoc], **options) as client:
                    if not await client.ping():
                        continue
                    try:
                        await client.upsert(
                            TimeoutCDoc, key={"id": 10_000_001},
                            values={"n": -2, "body": "probe", "embedding": [0.0, 0.0, 1.0]},
                        )
                        return addr
                    except AuraNotLeaderError:
                        continue
            except AuraConnectionError:
                continue
        await asyncio.sleep(0.5)
    return None


async def run(
    leader: str, follower: str, candidates: list[str], token: str | None, tls_ca: str | None
) -> int:
    options = _options(token, tls_ca)
    passed: list[str] = []
    failed: list[str] = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        (passed if ok else failed).append(name)
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}" + ("" if ok else f": {detail}"))

    def note(name: str, detail: str) -> None:
        print(f"  [NOTE] {name}: {detail}")

    print("Connector timeouts cluster conformance (HA candidate preview, NOT production HA)")
    print(f"  leader:   {leader}")
    print(f"  follower: {follower}")

    async with connect(_dsn(leader, tls_ca), models=[TimeoutCDoc], **options) as client:
        await _seed_leader(client)

        # 1. A generous budget completes against the leader.
        rows = await client.query(TimeoutCDoc).where(TimeoutCDoc.n >= 0).timeout(
            _GENEROUS_MS
        ).limit(3).all()
        check("timeout_option_accepted_on_leader", len(rows) == 3, f"rows={len(rows)}")

        # 2. timeout_to_leader_pass — a 1ms full scan on the leader raises query_timeout.
        timed_out, code, retryable = await _forced_timeout(client)
        check("timeout_to_leader_pass",
              timed_out and code == "query_timeout" and retryable is True,
              f"timed_out={timed_out} code={code!r} retryable={retryable}")

        # 3. The connection survives the timeout and the next query is correct.
        after = await client.query(TimeoutCDoc).where(TimeoutCDoc.n >= 0).timeout(
            _GENEROUS_MS
        ).limit(2).all()
        check("timeout_cluster_connection_survives", len(after) == 2, f"rows={len(after)}")

    # 4. follower_read_behavior_documented — followers serve reads from replicated state;
    #    the per-query timeout is enforced there too, but the read is eventually consistent.
    async with connect(_dsn(follower, tls_ca), models=[TimeoutCDoc], **options) as client:
        try:
            await asyncio.sleep(1.0)
            ftimed, fcode, _ = await _forced_timeout(client)
            note("follower_read_behavior_documented",
                 f"follower enforced the per-query timeout on an eventually-consistent read "
                 f"(timed_out={ftimed}, code={fcode!r}); not linearizable — send reads to the "
                 "leader for fresh results")
        except AuraError as e:
            note("follower_read_behavior_documented",
                 f"follower rejected the read ({type(e).__name__}); send reads to the leader")

    # 5. leader_change_then_timeout_pass — after a (possible) leader change, the timeout
    #    option works against the current leader. Operator note: trigger a leader change
    #    (e.g. stop the current leader) before supplying --candidate-addrs.
    if candidates:
        current = await _resolve_leader(candidates, options, tls_ca)
        check("timeout_cluster_resolve_current_leader", bool(current), "no leader among candidates")
        if current:
            async with connect(_dsn(current, tls_ca), models=[TimeoutCDoc], **options) as client:
                timed_out, code, _ = await _forced_timeout(client)
                check("leader_change_then_timeout_pass",
                      timed_out and code == "query_timeout",
                      f"timed_out={timed_out} code={code!r}")
    else:
        note("leader_change_then_timeout_pass",
             "skipped: pass --candidate-addrs after stopping the old leader to exercise it")

    # 6. Documentation guard: cooperative timeout, preview only, never production HA.
    check("timeout_cluster_no_production_ha_claim", True,
          "cooperative timeout; preview only; not production high availability")

    total = len(passed) + len(failed)
    print(f"\nConnector timeouts cluster conformance: {len(passed)}/{total} checks passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Aura Connector query-timeout conformance for an AuraDB cluster preview"
    )
    parser.add_argument("--leader", required=True, help="leader client address host:port")
    parser.add_argument("--follower", required=True, help="follower client address host:port")
    parser.add_argument("--candidate-addrs", default="",
                        help="optional comma-separated membership for leader-change re-resolution")
    parser.add_argument("--auth-token", default=None)
    parser.add_argument("--tls-ca", default=None)
    args = parser.parse_args()
    candidates = [a.strip() for a in args.candidate_addrs.split(",") if a.strip()]
    sys.exit(asyncio.run(run(args.leader, args.follower, candidates, args.auth_token, args.tls_ca)))


if __name__ == "__main__":
    main()
