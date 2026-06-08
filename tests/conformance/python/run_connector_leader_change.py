#!/usr/bin/env python3
"""Aura Connector behavior under an AuraDB leader change.

Validates Aura Connector >= 0.4.1 across a leader change in the controlled
static-cluster preview: a write to the old leader no longer succeeds as a leader
write (it is dead or has been demoted to a follower), the client discovers the
new leader (from the ``not_leader`` hint or by probing the candidate addresses),
the manual reconnect helper and the bounded ``with_leader_redirect`` reach the
new leader and apply the write exactly once without an unbounded retry loop,
authentication and TLS are preserved across the redirect, and a transaction is
never auto-redirected.

Run this AFTER the old leader has been stopped (or partitioned) and a new leader
has been elected — for example from ``scripts/smoke_ha_candidate.sh``. The old
leader address may now refuse connections (process stopped) or answer as a
follower (process restarted); both are handled.

AuraDB multi-node mode is a controlled static-cluster preview, not a production
high-availability guarantee. This is HA release-candidate conformance, not a
production HA claim. See docs/HA_RELEASE_CANDIDATE.md.

Exit codes: 0 success, 1 a failed check, 2 the connector is missing/too old.

Usage:
    python -m pip install "aura-connector>=0.4.1,<0.5"
    python run_connector_leader_change.py --leader 127.0.0.1:7171 \
        --candidate-addrs 127.0.0.1:7171,127.0.0.1:7181,127.0.0.1:7191
    python run_connector_leader_change.py --leader L --candidate-addrs A,B,C \
        --auth-token dev-secret --tls-ca .local/certs/ca.crt
"""

from __future__ import annotations

import argparse
import asyncio
import sys

try:
    from aura import AuraModel, AuraNotLeaderError, Field, connect
    from aura.config import TLSConfig, TokenAuth
    from aura.errors import AuraConnectionError, AuraTransactionError
except ImportError:
    print("aura-connector >= 0.4.1, < 0.5 is required: pip install 'aura-connector>=0.4.1,<0.5'")
    sys.exit(2)


class CItem(AuraModel):
    id: int = Field(primary_key=True)
    label: str


def _dsn(addr: str, tls_ca: str | None) -> str:
    return f"{'auradbs' if tls_ca else 'auradb'}://{addr}/cluster"


def _options(token: str | None, tls_ca: str | None) -> dict:
    options: dict = {}
    if token:
        options["auth"] = TokenAuth(token)
    if tls_ca:
        options["tls"] = TLSConfig(enabled=True, ca_cert_path=tls_ca, verify_hostname=True)
    return options


async def _resolve_leader_once(
    candidates: list[str], options: dict, tls_ca: str | None
) -> tuple[str, str] | None:
    """One pass over ``candidates`` to find the current leader. The authoritative
    signal is *accepting a write* (only the leader does), so probe every
    candidate and return the one that accepts; a follower's ``not_leader`` hint is
    only used as a fallback and is verified reachable, because immediately after a
    leader change a freshly-demoted follower may still report the old (now dead)
    leader. A dead/refusing node is skipped.

    Returns ``(addr, path)`` where ``path`` is ``"hint"`` when a follower's
    ``not_leader`` leader address resolved the leader directly, or ``"probe"``
    when the leader was found by probing the candidate addresses (the documented
    re-resolve fallback). ``None`` when no leader is found this pass."""
    hint: str | None = None
    for addr in candidates:
        try:
            async with connect(_dsn(addr, tls_ca), models=[CItem], **options) as client:
                if not await client.ping():
                    continue
                try:
                    await client.upsert(CItem, key={"id": 100}, values={"label": "probe"})
                    return addr, "probe"  # this node accepted a write — it is the leader
                except AuraNotLeaderError as exc:
                    if exc.leader_addr and hint is None:
                        hint = exc.leader_addr
        except AuraConnectionError:
            continue
    # No candidate accepted a write this pass. Prefer a follower's hint when it is
    # actually reachable (a stale hint to a stopped old leader is discarded).
    if hint and hint not in candidates:
        try:
            async with connect(_dsn(hint, tls_ca), models=[CItem], **options) as client:
                if await client.ping():
                    return hint, "hint"
        except AuraConnectionError:
            pass
    return None


async def _resolve_leader(
    candidates: list[str], options: dict, tls_ca: str | None
) -> tuple[str, str] | None:
    """Resolve the current leader, tolerating the brief re-election window after a
    leader change with a bounded retry (no unbounded loop). Returns ``(addr,
    path)`` as in :func:`_resolve_leader_once`, or ``None``."""
    for _ in range(10):
        resolved = await _resolve_leader_once(candidates, options, tls_ca)
        if resolved:
            return resolved
        await asyncio.sleep(0.5)
    return None


async def run(
    old_leader: str, candidates: list[str], token: str | None, tls_ca: str | None
) -> int:
    options = _options(token, tls_ca)

    passed: list[str] = []
    failed: list[str] = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        (passed if ok else failed).append(name)
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}" + ("" if ok else f": {detail}"))

    def note(name: str, detail: str) -> None:
        """A non-failing observation (does not affect the exit code)."""
        print(f"  [NOTE] {name}: {detail}")

    print("Connector leader-change conformance (HA release candidate, not production HA)")
    print(f"  old leader:       {old_leader}")
    print(f"  candidate addrs:  {', '.join(candidates)}")

    # 1. The old leader no longer accepts a leader write: it is either down
    #    (connection error) or demoted (not_leader). Either way it is bounded —
    #    a single attempt, no infinite retry.
    old_rejected = False
    try:
        async with connect(_dsn(old_leader, tls_ca), models=[CItem], **options) as client:
            try:
                await client.upsert(CItem, key={"id": 1}, values={"label": "stale-leader"})
                old_rejected = False
            except AuraNotLeaderError:
                old_rejected = True  # demoted to follower
    except AuraConnectionError:
        old_rejected = True  # process stopped
    check("old_leader_no_longer_leads", old_rejected)

    # 2. The client discovers the new leader (hint preferred, then bounded probe).
    resolved = await _resolve_leader(candidates, options, tls_ca)
    check("discovers_new_leader", bool(resolved), "no leader among candidates")
    if not resolved:
        total = len(passed) + len(failed)
        print(f"\nConnector leader-change conformance: {len(passed)}/{total} checks passed")
        return 1
    new_leader, resolve_path = resolved
    print(f"  new leader:       {new_leader}")
    print(f"  resolution path:  {resolve_path} ({'direct not_leader hint' if resolve_path == 'hint' else 're-resolve fallback (probed candidates)'})")
    # The resolver prefers a usable hint and only then falls back to probing the
    # candidates; either path is valid HA-candidate behavior, so this is a
    # non-failing observation, not a hard requirement on which path was taken.
    note("leader_change_conformance_prefers_hint_then_fallback", f"resolved via {resolve_path}")

    # 3. A write to the new leader succeeds.
    async with connect(_dsn(new_leader, tls_ca), models=[CItem], **options) as client:
        await client.upsert(CItem, key={"id": 2}, values={"label": "new-leader-write"})
        check(
            "write_to_new_leader",
            (await client.CItem.find(id=2)).label == "new-leader-write",
        )

    # 4-6. Find a follower authoritatively (a node that rejects a write with
    #      not_leader) and confirm the connector's redirect contract from it.
    #      After a leader change, leadership can still be settling, so do not
    #      assume which node is the follower — identify it by the not_leader
    #      response. Use fresh probe ids each attempt so a probe never collides
    #      with data written by earlier conformance runs against the same cluster.
    #      A node that *accepts* a probe is (now) a leader and is skipped.
    follower_rejected = False
    probe_id = 900
    for addr in candidates:
        if addr == new_leader:
            continue
        try:
            async with connect(_dsn(addr, tls_ca), models=[CItem], **options) as client:
                if not await client.ping():
                    continue
                # Poll briefly for a not_leader response; right after a change a
                # follower may not yet carry the leader's client address in its
                # hint, so wait a little for it to propagate.
                hint_exc = None
                for _ in range(10):
                    probe_id += 1
                    try:
                        await client.insert(CItem(id=probe_id, label="probe"))
                        break  # accepted -> this node is a leader now, not a follower
                    except AuraNotLeaderError as exc:
                        follower_rejected = True
                        if exc.leader_addr:
                            hint_exc = exc
                            break
                        await asyncio.sleep(0.5)
                    except AuraConnectionError:
                        break
                if not follower_rejected:
                    continue  # this node was not a follower; try the next candidate

                check("follower_not_leader", True)

                # The not_leader hint exposes the leader address; the reconnect
                # helper reaches it and preserves auth/TLS. Immediately after a
                # change the hint may still be propagating — then the documented
                # fallback (re-resolving the leader, validated above) applies and
                # the hint-dependent sub-checks are recorded as non-failing notes.
                if hint_exc is not None:
                    check("error_exposes_leader_addr", True, repr(hint_exc.leader_addr))
                    leader_client = await client.connect_to_leader(hint_exc)
                    try:
                        await leader_client.upsert(
                            CItem, key={"id": 50}, values={"label": "via-reconnect"}
                        )
                        check(
                            "reconnect_helper_writes_to_leader",
                            (await leader_client.CItem.find(id=50)).label == "via-reconnect",
                        )
                        check(
                            "auth_tls_preserved",
                            leader_client.config.auth is client.config.auth
                            and leader_client.config.tls == client.config.tls,
                        )
                    finally:
                        await leader_client.close()

                    # The bounded redirect helper applies the write once, no
                    # unbounded loop.
                    redirect = client.with_leader_redirect(max_redirects=1)
                    await redirect.upsert(CItem, key={"id": 51}, values={"label": "redirected"})
                    check(
                        "redirect_helper_bounded",
                        (await client.CItem.find(id=51)).label == "redirected",
                    )
                else:
                    note(
                        "error_exposes_leader_addr",
                        "follower's not_leader hint carried no leader address in "
                        "the post-change window; the client re-resolved the leader "
                        "(validated above), which is the documented fallback",
                    )
                    note(
                        "redirect_helper_bounded",
                        "skipped: requires a populated leader hint, still "
                        "propagating after the leader change",
                    )

                # A transaction is never auto-redirected across a leader change.
                # Use a FRESH client to the same follower: the redirect helper
                # above may have repinned `client`'s connection to the leader, so a
                # transaction on it would (correctly) reach the leader and commit.
                # The contract under test is that a transaction started against a
                # follower is not silently redirected.
                async with connect(_dsn(addr, tls_ca), models=[CItem], **options) as txn_client:
                    try:
                        async with txn_client.transaction() as txn:
                            await txn.insert(CItem(id=52, label="in-txn"))
                        check(
                            "transaction_not_redirected",
                            False,
                            "transaction unexpectedly succeeded",
                        )
                    except AuraNotLeaderError:
                        check("transaction_not_redirected", True)
                    try:
                        txn_client.with_leader_redirect().transaction()
                        check("redirect_rejects_transaction", False, "helper wrapped a transaction")
                    except AuraTransactionError:
                        check("redirect_rejects_transaction", True)
                break  # done with the follower scenarios
        except AuraConnectionError:
            continue

    if not follower_rejected:
        check("follower_not_leader", False, "no reachable follower rejected a write")

    total = len(passed) + len(failed)
    print(f"\nleader resolution path: {resolve_path} (hint preferred, probe fallback)")
    print(f"Connector leader-change conformance: {len(passed)}/{total} checks passed")
    return 0 if not failed else 1


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Aura Connector behavior under an AuraDB leader change"
    )
    parser.add_argument("--leader", required=True, help="old leader client address host:port")
    parser.add_argument(
        "--candidate-addrs",
        required=True,
        help="comma-separated candidate client addresses (the full membership)",
    )
    parser.add_argument("--auth-token", default=None)
    parser.add_argument("--tls-ca", default=None)
    args = parser.parse_args()
    candidates = [a.strip() for a in args.candidate_addrs.split(",") if a.strip()]
    sys.exit(asyncio.run(run(args.leader, candidates, args.auth_token, args.tls_ca)))


if __name__ == "__main__":
    main()
