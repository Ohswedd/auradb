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
# `.local/soak/logs` and are always retained. Per-node server logs are kept on
# failure for inspection (set KEEP_ARTIFACTS=1 to keep them on success too).
#
# Usage:
#   scripts/soak_cluster_preview.sh [DURATION_SECONDS]
#   SOAK_DURATION_SECS=300 scripts/soak_cluster_preview.sh
#   LEADER_ADDR=127.0.0.1:7171 SOAK_DURATION_SECS=10 scripts/soak_cluster_preview.sh
#
# Environment:
#   SOAK_DURATION_SECS   soak duration in seconds (default 120; positional arg wins)
#   LEADER_ADDR          leader client address to poll (default 127.0.0.1:7171)
#   KEEP_ARTIFACTS=1     keep per-node server logs after a successful run
#
# Default duration is 120 seconds.

set -euo pipefail

DURATION="${1:-${SOAK_DURATION_SECS:-120}}"
LEADER_ADDR="${LEADER_ADDR:-127.0.0.1:7171}"
KEEP_ARTIFACTS="${KEEP_ARTIFACTS:-0}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

LOG_DIR=".local/soak/logs"
mkdir -p "$LOG_DIR"
LOG="$LOG_DIR/cluster_preview_$(date +%s).log"

# Timestamped logger: every line is dated and tee'd to the run log.
log() { printf '%s %s\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$*" | tee -a "$LOG"; }

export PATH="$HOME/.cargo/bin:$PATH"
START="$(date +%s)"
log "cluster-preview soak starting (duration=${DURATION}s, leader=${LEADER_ADDR})"
log "log dir: $LOG_DIR"

log "building auradb CLI..."
cargo build -q -p auradb-cli
AURADB="$ROOT/target/debug/auradb"
log "binary version: $("$AURADB" version)"

rm -rf .local/cluster/node1 .local/cluster/node2 .local/cluster/node3

SUCCESS=0
PIDS=()
start_node() {
  local n="$1"
  "$AURADB" server --config "examples/cluster/node${n}.toml" \
    >".local/cluster-node${n}.log" 2>&1 &
  PIDS+=("$!")
}

cleanup() {
  log "stopping nodes..."
  for pid in "${PIDS[@]:-}"; do
    [[ -n "${pid}" ]] && kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  if [ "$SUCCESS" -eq 1 ] && [ "$KEEP_ARTIFACTS" != "1" ]; then
    rm -f .local/cluster-node1.log .local/cluster-node2.log .local/cluster-node3.log
  else
    log "per-node server logs preserved under .local/cluster-node*.log"
  fi
}
trap cleanup EXIT

for n in 1 2 3; do start_node "$n"; done

log "waiting for a leader..."
"$AURADB" cluster wait-leader --addr "$LEADER_ADDR" --timeout-secs 30 | tee -a "$LOG"

assert_healthy() {
  local addr="$1" json leader quorum
  json="$("$AURADB" cluster status --addr "$addr" --json 2>>"$LOG")" || {
    log "SOAK FAILED: cluster status query to $addr failed"
    return 1
  }
  leader="$(printf '%s' "$json" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d.get("leader_id") or "")')"
  quorum="$(printf '%s' "$json" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("quorum_available"))')"
  if [ -z "$leader" ]; then
    log "SOAK FAILED: no leader reported by $addr"
    printf '%s\n' "$json" | tee -a "$LOG"
    return 1
  fi
  if [ "$quorum" != "True" ]; then
    log "SOAK FAILED: quorum unavailable at $addr"
    return 1
  fi
  return 0
}

DEADLINE=$((START + DURATION))
RESTART_AT=$((START + DURATION / 2))
ITER=0
RESTARTED=0
log "soaking for ${DURATION}s..."
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  ITER=$((ITER + 1))
  assert_healthy "$LEADER_ADDR" || exit 1

  # Restart a follower once, midway through, then assert recovery.
  if [ "$RESTARTED" -eq 0 ] && [ "$(date +%s)" -ge "$RESTART_AT" ]; then
    log "restarting node2 to exercise reconnect/catch-up..."
    # node2 is the last-started among PIDS index 1.
    kill "${PIDS[1]}" 2>/dev/null || true
    sleep 2
    start_node 2
    "$AURADB" cluster wait-leader --addr "$LEADER_ADDR" --timeout-secs 30 | tee -a "$LOG"
    assert_healthy "$LEADER_ADDR" || exit 1
    RESTARTED=1
  fi
  sleep 2
done

END="$(date +%s)"
ELAPSED=$((END - START))
SUCCESS=1
log "cluster preview soak OK: $ITER health checks in ${ELAPSED}s, follower restart recovered"
log "summary: result=PASS checks=$ITER elapsed_secs=$ELAPSED leader=$LEADER_ADDR log=$LOG"
