#!/usr/bin/env bash
#
# Single-node soak / repeatability harness (MANUAL — not a required CI gate).
#
# Repeatedly re-imports a genuine dataset into a data directory and cycles the
# real durability machinery — restore (upsert, which creates new MVCC versions),
# structured `check`, `stats analyze`, `gc`, and `compact` — for a bounded
# duration, asserting after every cycle that:
#
#   * `auradb check --json` still reports ok = true, and
#   * the live record count stays exactly stable (upserts must not duplicate or
#     lose records).
#
# Exits non-zero on the first mismatch. Logs are captured under `.local/soak`.
# On success the data directory is removed (set KEEP_ARTIFACTS=1 to keep it); on
# failure everything is preserved for inspection. Logs are always retained.
#
# Usage:
#   scripts/soak_single_node.sh [DURATION_SECONDS]
#   SOAK_DURATION_SECS=300 scripts/soak_single_node.sh
#   SOAK_DURATION_SECS=10 KEEP_ARTIFACTS=1 scripts/soak_single_node.sh
#
# Environment:
#   SOAK_DURATION_SECS   soak duration in seconds (default 120; positional arg wins)
#   KEEP_ARTIFACTS=1     keep the data directory and seed after a successful run
#
# Default duration is 120 seconds.

set -euo pipefail

DURATION="${1:-${SOAK_DURATION_SECS:-120}}"
KEEP_ARTIFACTS="${KEEP_ARTIFACTS:-0}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SOAK_DIR=".local/soak"
DATA_DIR="$SOAK_DIR/single/data"
LOG_DIR="$SOAK_DIR/logs"
SEED="$SOAK_DIR/seed.jsonl"
LOG="$LOG_DIR/single_node_$(date +%s).log"

mkdir -p "$LOG_DIR"
rm -rf "$SOAK_DIR/single"
mkdir -p "$DATA_DIR"

# Timestamped logger: every line is dated and tee'd to the run log.
log() { printf '%s %s\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$*" | tee -a "$LOG"; }

SUCCESS=0
cleanup() {
  if [ "$SUCCESS" -eq 1 ] && [ "$KEEP_ARTIFACTS" != "1" ]; then
    rm -rf "$SOAK_DIR/single" "$SEED"
    log "cleaned data dir (KEEP_ARTIFACTS=1 to retain); log kept at $LOG"
  else
    log "artifacts preserved under $SOAK_DIR (data dir: $DATA_DIR, log: $LOG)"
  fi
}
trap cleanup EXIT

export PATH="$HOME/.cargo/bin:$PATH"
START="$(date +%s)"
log "single-node soak starting (duration=${DURATION}s, keep_artifacts=${KEEP_ARTIFACTS})"
log "data dir: $DATA_DIR"
log "log dir:  $LOG_DIR"

log "building auradb CLI..."
cargo build -q -p auradb-cli
BIN="$ROOT/target/debug/auradb"
log "binary version: $("$BIN" version)"

# Produce a genuine JSONL seed from a committed fixture (no hand-crafted data).
"$BIN" dump --data-dir tests/fixtures/v0_3_0_data --out "$SEED" >>"$LOG" 2>&1

# Establish the baseline record count.
"$BIN" restore --data-dir "$DATA_DIR" --input "$SEED" >>"$LOG" 2>&1
EXPECTED="$("$BIN" check --data-dir "$DATA_DIR" --json | python3 -c 'import sys,json;print(json.load(sys.stdin)["storage"]["records"])')"
log "baseline records: $EXPECTED"

check_ok() {
  local json records ok
  json="$("$BIN" check --data-dir "$DATA_DIR" --json)"
  ok="$(printf '%s' "$json" | python3 -c 'import sys,json;print(json.load(sys.stdin)["ok"])')"
  records="$(printf '%s' "$json" | python3 -c 'import sys,json;print(json.load(sys.stdin)["storage"]["records"])')"
  if [ "$ok" != "True" ]; then
    log "SOAK FAILED: check reported ok=$ok"
    printf '%s\n' "$json" | tee -a "$LOG"
    return 1
  fi
  if [ "$records" != "$EXPECTED" ]; then
    log "SOAK FAILED: record count drifted: expected $EXPECTED, got $records"
    return 1
  fi
  return 0
}

DEADLINE=$((START + DURATION))
ITER=0
log "soaking for ${DURATION}s..."
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  ITER=$((ITER + 1))
  # Re-import (upsert): churns MVCC versions without changing the live set.
  "$BIN" restore --data-dir "$DATA_DIR" --input "$SEED" >>"$LOG" 2>&1
  check_ok || exit 1
  # Periodically run the operational maintenance commands.
  if [ $((ITER % 3)) -eq 0 ]; then
    "$BIN" stats analyze --data-dir "$DATA_DIR" >>"$LOG" 2>&1
    "$BIN" gc --data-dir "$DATA_DIR" >>"$LOG" 2>&1
  fi
  if [ $((ITER % 5)) -eq 0 ]; then
    "$BIN" compact --data-dir "$DATA_DIR" >>"$LOG" 2>&1
    check_ok || exit 1
  fi
done

END="$(date +%s)"
ELAPSED=$((END - START))
SUCCESS=1
log "soak OK: $ITER cycles in ${ELAPSED}s, records stable at $EXPECTED"
log "summary: result=PASS cycles=$ITER elapsed_secs=$ELAPSED records=$EXPECTED log=$LOG"
