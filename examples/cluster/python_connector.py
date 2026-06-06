#!/usr/bin/env python3
"""Aura Connector against the Docker three-node preview cluster.

EXPERIMENTAL multi-node preview — there is no production high availability or
automatic failover, and single-node mode remains the recommended production path.
This script shows how a Python client behaves against the cluster started by
``docker-compose.cluster.yml``: only the leader accepts writes, and a write to a
follower raises ``AuraNotLeaderError``.

The compose file publishes each node's client port on the host (node1 → 7171,
node2 → 7181, node3 → 7191) with a plaintext, no-auth client listener. The
cluster's *internal* leader address (a ``node2:7171``-style name, or "unknown"
when peers do not declare a client address) is not reachable from the host, so on
the host you locate the leader by trying the published endpoints — this script
does exactly that, catching ``AuraNotLeaderError`` and moving to the next endpoint
rather than blindly auto-retrying a write whose application status is ambiguous.

It requires Aura Connector >= 0.4 (``pip install "aura-connector>=0.4,<0.5"``)
and prints install guidance if the connector is missing.

Run (after `docker compose -f docker-compose.cluster.yml up -d` and a leader is
elected):

    python examples/cluster/python_connector.py
    AURADB_CLUSTER_ENDPOINTS=127.0.0.1:7171,127.0.0.1:7181,127.0.0.1:7191 \
        python examples/cluster/python_connector.py
"""

from __future__ import annotations

import asyncio
import os
import sys

try:
    from aura import AuraConnectionError, AuraModel, AuraNotLeaderError, Field, connect
except ImportError:
    print(
        "Aura Connector is not installed.\n"
        "  pip install 'aura-connector>=0.4,<0.5'\n"
        "Then start the cluster and re-run:\n"
        "  docker compose -f docker-compose.cluster.yml up -d\n"
        "  python examples/cluster/python_connector.py"
    )
    sys.exit(0)


class Item(AuraModel):
    id: int = Field(primary_key=True)
    label: str


def _endpoints() -> list[str]:
    raw = os.environ.get("AURADB_CLUSTER_ENDPOINTS")
    if raw:
        return [e.strip() for e in raw.split(",") if e.strip()]
    return ["127.0.0.1:7171", "127.0.0.1:7181", "127.0.0.1:7191"]


async def write_to_leader(endpoints: list[str]) -> bool:
    """Try each published endpoint until the leader accepts the write."""
    for addr in endpoints:
        dsn = f"auradb://{addr}/cluster"
        try:
            async with connect(dsn, models=[Item]) as client:
                await client.upsert(Item, key={"id": 1}, values={"label": "hello-cluster"})
                row = await client.Item.find(id=1)
                print(f"leader is {addr}: wrote and read back {row.label!r}")
                return True
        except AuraNotLeaderError as exc:
            # A follower rejected the write. exc.leader_addr names the leader's
            # *internal* client address (or is None when peers do not declare one);
            # from the host we cannot reach that directly, so we try the next
            # published endpoint instead of auto-retrying an ambiguous write.
            hint = exc.leader_addr or "unknown (run `auradb cluster leader`)"
            print(f"{addr} is not the leader (leader hint: {hint}); trying the next endpoint")
        except AuraConnectionError as exc:
            print(f"{addr} is not reachable ({exc.message}); trying the next endpoint")
    return False


async def main() -> int:
    endpoints = _endpoints()
    print("EXPERIMENTAL preview cluster — no production HA or automatic failover.")
    print(f"locating the leader among: {', '.join(endpoints)}")
    if await write_to_leader(endpoints):
        return 0
    print(
        "No endpoint accepted the write. Is a leader elected?\n"
        "  auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30\n"
        "  auradb cluster leader      --addr 127.0.0.1:7171"
    )
    return 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
