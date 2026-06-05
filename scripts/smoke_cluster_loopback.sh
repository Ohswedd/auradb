#!/usr/bin/env bash
# Three-node loopback multi-node preview smoke test.
#
# Starts three local AuraDB server processes from examples/cluster/node{1,2,3}.toml
# (plaintext loopback peer transport — no certificates needed), waits for a
# leader, reports cluster status, and tears the cluster down. This is the
# EXPERIMENTAL multi-node preview; single-node mode remains the recommended
# production mode.
#
# Usage:
#   bash scripts/smoke_cluster_loopback.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

echo "building auradb..."
cargo build -p auradb-cli >/dev/null
AURADB="${REPO_ROOT}/target/debug/auradb"

# Fresh data directories for a clean election.
rm -rf .local/cluster/node1 .local/cluster/node2 .local/cluster/node3

PIDS=()
cleanup() {
  echo "stopping nodes..."
  for pid in "${PIDS[@]:-}"; do
    [[ -n "${pid}" ]] && kill "${pid}" 2>/dev/null || true
  done
  wait 2>/dev/null || true
}
trap cleanup EXIT

for n in 1 2 3; do
  "${AURADB}" server --config "examples/cluster/node${n}.toml" >".local/cluster-node${n}.log" 2>&1 &
  PIDS+=("$!")
done

echo "waiting for a leader on 127.0.0.1:7171..."
"${AURADB}" cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30

echo "cluster status:"
"${AURADB}" cluster status --addr 127.0.0.1:7171 --json
"${AURADB}" cluster leader --addr 127.0.0.1:7171

echo
echo "three-node loopback preview smoke OK"
