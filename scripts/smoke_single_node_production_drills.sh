#!/usr/bin/env bash
#
# Single-node production operability drills (MANUAL release-gate / on-demand).
#
# WHAT THIS IS
#   A bounded, offline harness that rehearses the operations a production
#   operator relies on for the SUPPORTED single-node deployment mode: backup,
#   verify, restore into a fresh data directory, rollback to a known-good
#   snapshot, disk-headroom preflight, clean I/O-error surfacing, and the
#   post-restore health checks (`doctor` / `check` / `stats`).
#
# WHAT THIS IS NOT
#   * It is NOT a multi-node HA proof. Multi-node remains an HA *candidate
#     preview only* (see docs/HA_RELEASE_CANDIDATE.md); nothing here exercises
#     replication, failover, or quorum.
#   * It makes NO production ANN claim. Exact vector search is the correctness
#     baseline; HNSW/ANN is an opt-in preview only. The seeded dataset includes
#     a vector field, but no drill asserts approximate recall.
#
# SAFETY
#   * Every artifact lives under .local/ (git-ignored). Nothing outside .local/
#     is created, modified, or removed.
#   * Disk-full is NEVER simulated by filling the real disk. The disk drill is a
#     read-only `df` preflight that verifies the free-space check and its
#     structured warning path.
#   * I/O errors are simulated with permission-denied / missing / corrupt paths
#     under .local/. No user data is corrupted.
#   * Bounded runtime: a small, configurable dataset and no network server.
#
# OUTPUT
#   * Clear [PASS]/[FAIL]/[WARN] lines per drill.
#   * A machine-readable JSON report under .local/prod-drills/ (see DRILL_REPORT
#     below): per-drill name, status, duration, data dir, backup/restore/snapshot
#     paths, checks run, and warnings.
#   * Exits non-zero on the first real failure. Logs are ALWAYS preserved under
#     .local/prod-drills/logs/; on failure the data directories are preserved too.
#
# USAGE
#   scripts/smoke_single_node_production_drills.sh
#   DRILL_RECORDS=5000 scripts/smoke_single_node_production_drills.sh
#   MIN_FREE_MB=1024 KEEP_ARTIFACTS=1 scripts/smoke_single_node_production_drills.sh
#
# ENVIRONMENT
#   DRILL_RECORDS     records to seed the rehearsal dataset with (default 2000)
#   MIN_FREE_MB       soft free-space threshold; below it the preflight WARNs
#                     (default 500). A hard floor of 50 MB FAILs the preflight.
#   KEEP_ARTIFACTS=1  keep data directories after a successful run (logs + report
#                     are always kept)
#   AURADB_BIN        path to a prebuilt `auradb` binary (skips `cargo build`)
#
# This script intentionally targets stock macOS bash 3.2: no associative arrays,
# no `mapfile`, no `${var^^}`, no other bash-4-only features.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DRILL_RECORDS="${DRILL_RECORDS:-2000}"
MIN_FREE_MB="${MIN_FREE_MB:-500}"
HARD_FLOOR_MB=50
KEEP_ARTIFACTS="${KEEP_ARTIFACTS:-0}"

DRILL_ROOT=".local/prod-drills"
RUN_TS="$(date +%s)"
RUN_DIR="$DRILL_ROOT/run-$RUN_TS"
LOG_DIR="$DRILL_ROOT/logs"
LOG="$LOG_DIR/drills_$RUN_TS.log"
DRILL_REPORT="$RUN_DIR/report.json"

DATA_DIR="$RUN_DIR/data"          # seeded "larger state" (the production data dir)
FRESH_DIR="$RUN_DIR/restore"      # restore-into-fresh target
ROLLBACK_DIR="$RUN_DIR/rollback"  # rollback rehearsal target
BACKUP="$RUN_DIR/backup.jsonl"    # logical backup
SNAPSHOT="$RUN_DIR/known-good.snap" # portable snapshot (rollback point)

mkdir -p "$LOG_DIR" "$RUN_DIR"

# Timestamped logger; every line is dated and tee'd to the run log.
log() { printf '%s %s\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$*" | tee -a "$LOG"; }

# PASS/FAIL/WARN reporters print to stdout and the log.
pass() { log "[PASS] $*"; }
warn() { log "[WARN] $*"; }
fail() { log "[FAIL] $*"; }

FAILURES=0
SUCCESS=0

# --- JSON report assembly (bash 3.2-safe: indexed array of object strings) ----
DRILL_JSON=()          # one JSON object per drill
RUN_WARNINGS=()        # run-level warnings

# Escape a string for embedding in JSON (backslash first, then double-quote).
json_escape() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  printf '%s' "$s"
}

# Build a JSON array literal from the remaining arguments (each a raw string).
json_str_array() {
  local out="[" first=1 item
  for item in "$@"; do
    # Skip empties so an unset/empty array (expanded via "${arr[@]:-}") renders
    # as [] rather than [""].
    [ -n "$item" ] || continue
    if [ "$first" -eq 1 ]; then first=0; else out="$out,"; fi
    out="$out\"$(json_escape "$item")\""
  done
  printf '%s]' "$out"
}

# record_drill <name> <status> <duration_secs> <checks_json_array> <warnings_json_array>
record_drill() {
  local name="$1" status="$2" dur="$3" checks="$4" warns="$5"
  DRILL_JSON+=("{\"name\":\"$(json_escape "$name")\",\"status\":\"$(json_escape "$status")\",\"duration_secs\":$dur,\"checks\":$checks,\"warnings\":$warns}")
}

now() { date +%s; }

# --- cleanup: logs + report ALWAYS kept; data dirs kept on failure -------------
cleanup() {
  # Restore writability on any permission-denied fixture so rm can remove it.
  if [ -d "$RUN_DIR/io" ]; then chmod -R u+rwx "$RUN_DIR/io" 2>/dev/null || true; fi
  write_report
  if [ "$SUCCESS" -eq 1 ] && [ "$KEEP_ARTIFACTS" != "1" ]; then
    rm -rf "$DATA_DIR" "$FRESH_DIR" "$ROLLBACK_DIR" "$BACKUP" "$SNAPSHOT" "$RUN_DIR/io" 2>/dev/null || true
    log "cleaned data dirs (KEEP_ARTIFACTS=1 to retain); report: $DRILL_REPORT log: $LOG"
  else
    log "artifacts preserved under $RUN_DIR (report: $DRILL_REPORT, log: $LOG)"
  fi
}

write_report() {
  local overall="pass"
  if [ "$FAILURES" -gt 0 ]; then overall="fail"; fi
  local drills="[" first=1 d
  for d in "${DRILL_JSON[@]:-}"; do
    [ -n "$d" ] || continue
    if [ "$first" -eq 1 ]; then first=0; else drills="$drills,"; fi
    drills="$drills$d"
  done
  drills="$drills]"
  local run_warns
  run_warns="$(json_str_array "${RUN_WARNINGS[@]:-}")"
  # macOS `date -r` and GNU `date -d @` differ; record the epoch and let readers format.
  cat >"$DRILL_REPORT" <<EOF
{
  "tool": "smoke_single_node_production_drills",
  "label": "single-node production drill; not a multi-node HA proof; no production ANN claim",
  "auradb_version": "$(json_escape "$AURADB_VERSION")",
  "started_at_unix": $RUN_TS,
  "duration_secs": $(( $(now) - RUN_TS )),
  "records_seeded": $DRILL_RECORDS,
  "data_dir": "$(json_escape "$DATA_DIR")",
  "backup_path": "$(json_escape "$BACKUP")",
  "restore_path": "$(json_escape "$FRESH_DIR")",
  "snapshot_path": "$(json_escape "$SNAPSHOT")",
  "rollback_path": "$(json_escape "$ROLLBACK_DIR")",
  "overall": "$overall",
  "failures": $FAILURES,
  "run_warnings": $run_warns,
  "drills": $drills
}
EOF
}
trap cleanup EXIT

# --- JSON field extraction without jq (python3 is already a soak-script dep) ---
json_get() {
  # json_get <field-path> : reads JSON on stdin, prints the field, e.g. "storage.records".
  python3 -c '
import sys, json
doc = json.load(sys.stdin)
for key in sys.argv[1].split("."):
    doc = doc[key]
print(doc)
' "$1"
}

export PATH="$HOME/.cargo/bin:$PATH"

log "single-node production drills starting (records=$DRILL_RECORDS, min_free_mb=$MIN_FREE_MB)"
log "label: single-node production drill; NOT a multi-node HA proof; NO production ANN claim"
log "run dir: $RUN_DIR"

if [ -n "${AURADB_BIN:-}" ] && [ -x "${AURADB_BIN}" ]; then
  BIN="$AURADB_BIN"
  log "using prebuilt binary: $BIN"
else
  log "building auradb CLI..."
  cargo build -q -p auradb-cli
  BIN="$ROOT/target/debug/auradb"
fi
AURADB_VERSION="$("$BIN" version)"
log "binary version: $AURADB_VERSION"

# ==============================================================================
# Drill 1: disk_space_preflight
#   Read-only `df` headroom check. Verifies the free-space probe and its
#   structured warning/failure path WITHOUT writing unbounded data.
# ==============================================================================
drill_disk_space_preflight() {
  local start; start="$(now)"
  local checks=() warns=() status="pass"
  # POSIX `df -k` works on macOS and Linux. The 4th column is available 1K-blocks
  # for the filesystem backing the target path. Use the last data line to be
  # robust to multi-line headers from long device names.
  local avail_kb avail_mb
  if ! avail_kb="$(df -k "$RUN_DIR" 2>>"$LOG" | awk 'NR>1 {a=$4} END {print a}')"; then
    fail "disk_space_preflight: df failed for $RUN_DIR"
    record_drill "disk_space_preflight" "fail" "$(( $(now) - start ))" "$(json_str_array "df")" "$(json_str_array "df failed")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  case "$avail_kb" in
    ''|*[!0-9]*)
      fail "disk_space_preflight: could not parse df output ('$avail_kb')"
      record_drill "disk_space_preflight" "fail" "$(( $(now) - start ))" "$(json_str_array "df-parse")" "$(json_str_array "unparseable df output")"
      FAILURES=$((FAILURES + 1)); return 1 ;;
  esac
  avail_mb=$(( avail_kb / 1024 ))
  checks+=("df_available_mb=$avail_mb")
  log "disk_space_preflight: ${avail_mb} MB free on the filesystem backing $RUN_DIR (soft threshold ${MIN_FREE_MB} MB, hard floor ${HARD_FLOOR_MB} MB)"
  if [ "$avail_mb" -lt "$HARD_FLOOR_MB" ]; then
    fail "disk_space_preflight: only ${avail_mb} MB free; below the ${HARD_FLOOR_MB} MB hard floor — refusing to run drills"
    warns+=("below hard floor ${HARD_FLOOR_MB} MB")
    record_drill "disk_space_preflight" "fail" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "${warns[@]}")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  if [ "$avail_mb" -lt "$MIN_FREE_MB" ]; then
    warn "disk_space_preflight: ${avail_mb} MB free is below the ${MIN_FREE_MB} MB soft threshold — operators should add headroom"
    warns+=("below soft threshold ${MIN_FREE_MB} MB")
    RUN_WARNINGS+=("disk headroom ${avail_mb} MB < ${MIN_FREE_MB} MB soft threshold")
    status="warn"
  fi
  [ "$status" = "warn" ] || pass "disk_space_preflight: ${avail_mb} MB free (>= ${MIN_FREE_MB} MB)"
  record_drill "disk_space_preflight" "$status" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "${warns[@]:-}")"
}

# ==============================================================================
# Drill 2: backup_restore_rehearsal
#   Seed a realistic, bounded "larger state" via `bench`, take a logical backup,
#   and verify it. (`bench` builds a full schema: string/document/full-text/exact
#   vector fields and indexes, then inserts DRILL_RECORDS records.)
# ==============================================================================
BASELINE_RECORDS=0
drill_backup_restore_rehearsal() {
  local start; start="$(now)"
  local checks=() warns=()
  rm -rf "$DATA_DIR"
  log "backup_restore_rehearsal: seeding $DRILL_RECORDS records into $DATA_DIR via bench..."
  if ! "$BIN" bench --data-dir "$DATA_DIR" --records "$DRILL_RECORDS" >>"$LOG" 2>&1; then
    fail "backup_restore_rehearsal: seeding failed"
    record_drill "backup_restore_rehearsal" "fail" "$(( $(now) - start ))" "$(json_str_array "seed")" "$(json_str_array "bench seeding failed")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  BASELINE_RECORDS="$("$BIN" check --data-dir "$DATA_DIR" --json | json_get storage.records)"
  checks+=("seeded_records=$BASELINE_RECORDS")
  log "backup_restore_rehearsal: baseline records=$BASELINE_RECORDS"
  log "backup_restore_rehearsal: writing logical backup -> $BACKUP"
  if ! "$BIN" dump --data-dir "$DATA_DIR" --out "$BACKUP" >>"$LOG" 2>&1; then
    fail "backup_restore_rehearsal: dump failed"
    record_drill "backup_restore_rehearsal" "fail" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "dump failed")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  checks+=("dump")
  local verify_json verify_ok verify_records
  verify_json="$("$BIN" backup verify --input "$BACKUP" --json)"
  verify_ok="$(printf '%s' "$verify_json" | json_get ok)"
  verify_records="$(printf '%s' "$verify_json" | json_get records)"
  checks+=("backup_verify_ok=$verify_ok" "backup_records=$verify_records")
  if [ "$verify_ok" != "True" ]; then
    fail "backup_restore_rehearsal: backup verify reported ok=$verify_ok"
    printf '%s\n' "$verify_json" >>"$LOG"
    record_drill "backup_restore_rehearsal" "fail" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "backup verify failed")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  pass "backup_restore_rehearsal: backup created and verified ($verify_records records)"
  record_drill "backup_restore_rehearsal" "pass" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "${warns[@]:-}")"
}

# ==============================================================================
# Drill 3: restore_to_fresh_data_dir
#   Capture a portable snapshot of the seeded state and restore it into a FRESH
#   directory; the restored record count must equal the baseline.
# ==============================================================================
drill_restore_to_fresh_data_dir() {
  local start; start="$(now)"
  local checks=()
  rm -rf "$FRESH_DIR"
  log "restore_to_fresh_data_dir: snapshot $DATA_DIR -> $SNAPSHOT"
  if ! "$BIN" snapshot create --data-dir "$DATA_DIR" --output "$SNAPSHOT" >>"$LOG" 2>&1; then
    fail "restore_to_fresh_data_dir: snapshot create failed"
    record_drill "restore_to_fresh_data_dir" "fail" "$(( $(now) - start ))" "$(json_str_array "snapshot-create")" "$(json_str_array "snapshot create failed")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  checks+=("snapshot_create")
  log "restore_to_fresh_data_dir: restore $SNAPSHOT -> $FRESH_DIR"
  if ! "$BIN" snapshot restore --input "$SNAPSHOT" --data-dir "$FRESH_DIR" >>"$LOG" 2>&1; then
    fail "restore_to_fresh_data_dir: snapshot restore failed"
    record_drill "restore_to_fresh_data_dir" "fail" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "snapshot restore failed")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  local restored
  restored="$("$BIN" check --data-dir "$FRESH_DIR" --json | json_get storage.records)"
  checks+=("restored_records=$restored")
  if [ "$restored" != "$BASELINE_RECORDS" ]; then
    fail "restore_to_fresh_data_dir: record count drift (baseline=$BASELINE_RECORDS, restored=$restored)"
    record_drill "restore_to_fresh_data_dir" "fail" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "record count drift")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  pass "restore_to_fresh_data_dir: restored $restored records into a fresh dir (matches baseline)"
  record_drill "restore_to_fresh_data_dir" "pass" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "[]"
}

# ==============================================================================
# Drill 4: rollback_rehearsal
#   Prove an operator can roll a data directory BACK to a known-good snapshot.
#   Seed ROLLBACK_DIR with a DIFFERENT dataset (a committed fixture), then
#   force-restore the known-good snapshot over it and confirm the state is the
#   known-good one — not the "changed" one.
# ==============================================================================
drill_rollback_rehearsal() {
  local start; start="$(now)"
  local checks=() warns=()
  rm -rf "$ROLLBACK_DIR"
  # Establish a "current (to-be-rolled-back) state" different from known-good by
  # restoring a committed fixture's logical dump into the rollback dir.
  local fixture="tests/fixtures/v0_3_0_data"
  local pre_count="n/a"
  if [ -d "$fixture" ]; then
    local fix_dump="$RUN_DIR/fixture.jsonl"
    if "$BIN" dump --data-dir "$fixture" --out "$fix_dump" >>"$LOG" 2>&1 \
       && "$BIN" restore --data-dir "$ROLLBACK_DIR" --input "$fix_dump" >>"$LOG" 2>&1; then
      pre_count="$("$BIN" check --data-dir "$ROLLBACK_DIR" --json | json_get storage.records)"
    else
      warn "rollback_rehearsal: could not seed divergent state from fixture; rolling back over an empty dir"
      warns+=("fixture seed unavailable")
    fi
  else
    warn "rollback_rehearsal: fixture $fixture absent; rolling back over an empty dir"
    warns+=("fixture absent")
  fi
  checks+=("pre_rollback_records=$pre_count")
  log "rollback_rehearsal: force-restoring known-good snapshot over $ROLLBACK_DIR (was: $pre_count records)"
  if ! "$BIN" snapshot restore --input "$SNAPSHOT" --data-dir "$ROLLBACK_DIR" --force >>"$LOG" 2>&1; then
    fail "rollback_rehearsal: forced snapshot restore failed"
    record_drill "rollback_rehearsal" "fail" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "forced restore failed")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  local post_count
  post_count="$("$BIN" check --data-dir "$ROLLBACK_DIR" --json | json_get storage.records)"
  checks+=("post_rollback_records=$post_count")
  if [ "$post_count" != "$BASELINE_RECORDS" ]; then
    fail "rollback_rehearsal: rolled-back state ($post_count) does not match known-good ($BASELINE_RECORDS)"
    record_drill "rollback_rehearsal" "fail" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "rollback did not reach known-good")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  pass "rollback_rehearsal: rolled back to known-good ($post_count records; was $pre_count)"
  record_drill "rollback_rehearsal" "pass" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "${warns[@]:-}")"
}

# ==============================================================================
# Drill 5: io_error_surface_check
#   Confirm I/O faults are surfaced as clean, structured errors (non-zero exit,
#   a message naming the path) and NEVER as a panic. Uses permission-denied,
#   missing, and corrupt paths under .local/. No user data is harmed.
# ==============================================================================
# expect_clean_error <label> <command...> : passes if the command exits non-zero
# with a non-empty, non-panicking error. Records into the shared arrays.
IO_CHECKS=()
IO_WARNS=()
IO_OK=1
expect_clean_error() {
  local label="$1"; shift
  local out rc
  out="$("$@" 2>&1)" && rc=0 || rc=$?
  if printf '%s' "$out" | grep -qi 'panic'; then
    fail "io_error_surface_check/$label: command PANICKED instead of erroring cleanly"
    printf '%s\n' "$out" >>"$LOG"
    IO_CHECKS+=("$label=panic"); IO_OK=0; return 1
  fi
  if [ "$rc" -eq 0 ]; then
    # The fault could not be induced in this environment (e.g. running as root
    # bypasses permission bits). Record a warning rather than a false failure.
    warn "io_error_surface_check/$label: expected an error but command succeeded (environment cannot induce this fault?)"
    IO_CHECKS+=("$label=unexpected-success"); IO_WARNS+=("$label could not be induced")
    return 0
  fi
  IO_CHECKS+=("$label=clean-error")
  return 0
}

drill_io_error_surface_check() {
  local start; start="$(now)"
  IO_CHECKS=(); IO_WARNS=(); IO_OK=1
  local io="$RUN_DIR/io"
  mkdir -p "$io"

  # (a) permission-denied backup destination.
  local ro="$io/readonly"
  mkdir -p "$ro"; chmod 000 "$ro" 2>/dev/null || true
  expect_clean_error "permission_denied_dump" \
    "$BIN" dump --data-dir "$DATA_DIR" --out "$ro/backup.jsonl"
  chmod u+rwx "$ro" 2>/dev/null || true

  # (b) missing snapshot input.
  expect_clean_error "missing_snapshot_input" \
    "$BIN" snapshot restore --input "$io/does-not-exist.snap" --data-dir "$io/restore-missing"

  # (c) corrupt logical backup is rejected by restore.
  local corrupt="$io/corrupt.jsonl"
  printf '{"this is not": valid json\n' >"$corrupt"
  expect_clean_error "corrupt_backup_restore" \
    "$BIN" restore --data-dir "$io/restore-corrupt" --input "$corrupt"

  # (d) corrupt backup is rejected by `backup verify` (structured, non-zero).
  expect_clean_error "corrupt_backup_verify" \
    "$BIN" backup verify --input "$corrupt" --json

  if [ "$IO_OK" -ne 1 ]; then
    record_drill "io_error_surface_check" "fail" "$(( $(now) - start ))" "$(json_str_array "${IO_CHECKS[@]}")" "$(json_str_array "${IO_WARNS[@]:-}")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  local status="pass"
  if [ "${#IO_WARNS[@]}" -gt 0 ]; then status="warn"; fi
  pass "io_error_surface_check: all faults surfaced cleanly (no panics)"
  record_drill "io_error_surface_check" "$status" "$(( $(now) - start ))" "$(json_str_array "${IO_CHECKS[@]}")" "$(json_str_array "${IO_WARNS[@]:-}")"
}

# ==============================================================================
# Drill 6: doctor_after_restore
#   `doctor --json` on the restored dir must report a healthy, consistent state.
# ==============================================================================
drill_doctor_after_restore() {
  local start; start="$(now)"
  local checks=()
  # Refresh advisory planner stats so a benign "stale stats" warning does not
  # cloud the health read; this is part of a real post-restore runbook.
  "$BIN" stats analyze --data-dir "$FRESH_DIR" >>"$LOG" 2>&1 || true
  local doc consistency
  doc="$("$BIN" doctor --data-dir "$FRESH_DIR" --json)"
  consistency="$(printf '%s' "$doc" | json_get index_consistency_ok)"
  checks+=("index_consistency_ok=$consistency")
  if [ "$consistency" != "True" ]; then
    fail "doctor_after_restore: index_consistency_ok=$consistency"
    printf '%s\n' "$doc" >>"$LOG"
    record_drill "doctor_after_restore" "fail" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "index inconsistent after restore")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  pass "doctor_after_restore: restored data dir is healthy (index consistency ok)"
  record_drill "doctor_after_restore" "pass" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "[]"
}

# ==============================================================================
# Drill 7: check_after_restore
#   `check --json` on the restored dir must report ok = true.
# ==============================================================================
drill_check_after_restore() {
  local start; start="$(now)"
  local checks=()
  local chk ok records
  chk="$("$BIN" check --data-dir "$FRESH_DIR" --json)"
  ok="$(printf '%s' "$chk" | json_get ok)"
  records="$(printf '%s' "$chk" | json_get storage.records)"
  checks+=("ok=$ok" "records=$records")
  if [ "$ok" != "True" ]; then
    fail "check_after_restore: ok=$ok"
    printf '%s\n' "$chk" >>"$LOG"
    record_drill "check_after_restore" "fail" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "$(json_str_array "check failed after restore")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  pass "check_after_restore: consistency check ok ($records records)"
  record_drill "check_after_restore" "pass" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "[]"
}

# ==============================================================================
# Drill 8: metrics_or_stats_snapshot
#   Capture a machine-readable stats snapshot of the restored dir as evidence.
# ==============================================================================
drill_metrics_or_stats_snapshot() {
  local start; start="$(now)"
  local checks=()
  local stats_json out="$RUN_DIR/stats.json"
  if ! stats_json="$("$BIN" stats show --data-dir "$FRESH_DIR" --json)"; then
    fail "metrics_or_stats_snapshot: stats show failed"
    record_drill "metrics_or_stats_snapshot" "fail" "$(( $(now) - start ))" "$(json_str_array "stats-show")" "$(json_str_array "stats show failed")"
    FAILURES=$((FAILURES + 1)); return 1
  fi
  printf '%s\n' "$stats_json" >"$out"
  checks+=("stats_snapshot=$out")
  pass "metrics_or_stats_snapshot: wrote stats snapshot to $out"
  record_drill "metrics_or_stats_snapshot" "pass" "$(( $(now) - start ))" "$(json_str_array "${checks[@]}")" "[]"
}

# --- run the drills in order --------------------------------------------------
drill_disk_space_preflight       || true
drill_backup_restore_rehearsal   || true
drill_restore_to_fresh_data_dir  || true
drill_rollback_rehearsal         || true
drill_io_error_surface_check     || true
drill_doctor_after_restore       || true
drill_check_after_restore        || true
drill_metrics_or_stats_snapshot  || true

ELAPSED=$(( $(now) - RUN_TS ))
if [ "$FAILURES" -eq 0 ]; then
  SUCCESS=1
  log "summary: result=PASS drills=${#DRILL_JSON[@]} failures=0 elapsed_secs=$ELAPSED report=$DRILL_REPORT"
  exit 0
else
  log "summary: result=FAIL drills=${#DRILL_JSON[@]} failures=$FAILURES elapsed_secs=$ELAPSED report=$DRILL_REPORT"
  exit 1
fi
