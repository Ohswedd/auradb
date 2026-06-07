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
# Exits non-zero on the first mismatch. Logs and a copy of the data directory
# state are captured under `.local/soak`. The data directory is cleaned on a
# successful run; logs are retained.
#
# Usage:
#   scripts/soak_single_node.sh [DURATION_SECONDS]
#   SOAK_DURATION_SECS=300 scripts/soak_single_node.sh
#
# Default duration is 120 seconds.

set -euo pipefail

DURATION="${1:-${SOAK_DURATION_SECS:-120}}"
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

export PATH="$HOME/.cargo/bin:$PATH"
echo "building auradb CLI..." | tee "$LOG"
cargo build -q -p auradb-cli
BIN="$ROOT/target/debug/auradb"

# Produce a genuine JSONL seed from a committed fixture (no hand-crafted data).
"$BIN" dump --data-dir tests/fixtures/v0_3_0_data --out "$SEED" >>"$LOG" 2>&1

# Establish the baseline record count.
"$BIN" restore --data-dir "$DATA_DIR" --input "$SEED" >>"$LOG" 2>&1
EXPECTED="$("$BIN" check --data-dir "$DATA_DIR" --json | python3 -c 'import sys,json;print(json.load(sys.stdin)["storage"]["records"])')"
echo "baseline records: $EXPECTED" | tee -a "$LOG"

check_ok() {
  local json records ok
  json="$("$BIN" check --data-dir "$DATA_DIR" --json)"
  ok="$(printf '%s' "$json" | python3 -c 'import sys,json;print(json.load(sys.stdin)["ok"])')"
  records="$(printf '%s' "$json" | python3 -c 'import sys,json;print(json.load(sys.stdin)["storage"]["records"])')"
  if [ "$ok" != "True" ]; then
    echo "SOAK FAILED: check reported ok=$ok" | tee -a "$LOG"
    printf '%s\n' "$json" | tee -a "$LOG"
    return 1
  fi
  if [ "$records" != "$EXPECTED" ]; then
    echo "SOAK FAILED: record count drifted: expected $EXPECTED, got $records" | tee -a "$LOG"
    return 1
  fi
  return 0
}

START="$(date +%s)"
DEADLINE=$((START + DURATION))
ITER=0
echo "soaking for ${DURATION}s..." | tee -a "$LOG"
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

echo "soak OK: $ITER cycles, records stable at $EXPECTED (log: $LOG)" | tee -a "$LOG"
# Clean the data dir on success; keep the log.
rm -rf "$SOAK_DIR/single" "$SEED"
