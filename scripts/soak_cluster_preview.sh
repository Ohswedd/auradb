#!/usr/bin/env bash
#
# Cluster-preview soak / repeatability harness (MANUAL — not a required CI gate).
#
# Brings up the three-node loopback multi-node *preview* (the same topology as
# scripts/smoke_cluster_loopback.sh), then for a bounded duration repeatedly
# polls live cluster diagnostics and asserts the cluster stays healthy:
#
#   * a leader is always present, and
#   * quorum stays available.
#
# Midway through, a follower is killed and restarted to exercise reconnect and
# catch-up; the harness then re-asserts that a leader and quorum recover.
#
# This is the EXPERIMENTAL multi-node preview. It is NOT production HA, and
# single-node mode remains the recommended production mode. A write workload
# requires the Aura Connector (Python) driving the leader's client port; this
# harness exercises the consensus/recovery path with reads/diagnostics only.
#
# Exits non-zero on the first failed health assertion. Logs are under
# `.local/soak/logs`.
#
# Usage:
#   scripts/soak_cluster_preview.sh [DURATION_SECONDS]
#   SOAK_DURATION_SECS=300 scripts/soak_cluster_preview.sh
#
# Default duration is 120 seconds.

set -euo pipefail

DURATION="${1:-${SOAK_DURATION_SECS:-120}}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

LOG_DIR=".local/soak/logs"
mkdir -p "$LOG_DIR"
LOG="$LOG_DIR/cluster_preview_$(date +%s).log"

export PATH="$HOME/.cargo/bin:$PATH"
echo "building auradb CLI..." | tee "$LOG"
cargo build -q -p auradb-cli
AURADB="$ROOT/target/debug/auradb"

rm -rf .local/cluster/node1 .local/cluster/node2 .local/cluster/node3

PIDS=()
start_node() {
  local n="$1"
  "$AURADB" server --config "examples/cluster/node${n}.toml" \
    >".local/cluster-node${n}.log" 2>&1 &
  PIDS+=("$!")
}

cleanup() {
  echo "stopping nodes..." | tee -a "$LOG"
  for pid in "${PIDS[@]:-}"; do
    [[ -n "${pid}" ]] && kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
}
trap cleanup EXIT

for n in 1 2 3; do start_node "$n"; done

echo "waiting for a leader..." | tee -a "$LOG"
"$AURADB" cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30 | tee -a "$LOG"

assert_healthy() {
  local addr="$1" json leader quorum
  json="$("$AURADB" cluster status --addr "$addr" --json 2>>"$LOG")" || {
    echo "SOAK FAILED: cluster status query to $addr failed" | tee -a "$LOG"
    return 1
  }
  leader="$(printf '%s' "$json" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d.get("leader_id") or "")')"
  quorum="$(printf '%s' "$json" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("quorum_available"))')"
  if [ -z "$leader" ]; then
    echo "SOAK FAILED: no leader reported by $addr" | tee -a "$LOG"
    printf '%s\n' "$json" | tee -a "$LOG"
    return 1
  fi
  if [ "$quorum" != "True" ]; then
    echo "SOAK FAILED: quorum unavailable at $addr" | tee -a "$LOG"
    return 1
  fi
  return 0
}

START="$(date +%s)"
DEADLINE=$((START + DURATION))
RESTART_AT=$((START + DURATION / 2))
ITER=0
RESTARTED=0
echo "soaking for ${DURATION}s..." | tee -a "$LOG"
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  ITER=$((ITER + 1))
  assert_healthy 127.0.0.1:7171 || exit 1

  # Restart a follower once, midway through, then assert recovery.
  if [ "$RESTARTED" -eq 0 ] && [ "$(date +%s)" -ge "$RESTART_AT" ]; then
    echo "restarting node2 to exercise reconnect/catch-up..." | tee -a "$LOG"
    # node2 is the last-started among PIDS index 1.
    kill "${PIDS[1]}" 2>/dev/null || true
    sleep 2
    start_node 2
    "$AURADB" cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 30 | tee -a "$LOG"
    assert_healthy 127.0.0.1:7171 || exit 1
    RESTARTED=1
  fi
  sleep 2
done

echo "cluster preview soak OK: $ITER health checks, follower restart recovered (log: $LOG)" | tee -a "$LOG"
