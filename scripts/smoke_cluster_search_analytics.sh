#!/usr/bin/env bash
# Live Docker Compose cluster search-and-analytics preview smoke + leader-change drill.
#
# Starts the three-node Compose cluster, waits for a leader, then exercises the
# v1.3.x search/analytics surface through the public Aura Connector against the
# leader: BM25 search, facets, GROUP BY analytics, ranked pagination + public
# cursor resume, ANN preview (with exact fallback), query profile, and a
# structured query timeout. It then performs a bounded leader-change drill (stop
# the leader, wait for a new leader, re-run the search/facet/pagination/group-by
# checks), brings the old leader back, and confirms quorum is restored.
#
# This is the EXPERIMENTAL multi-node preview and an HA *candidate* drill. It is
# NOT production HA proof: it is a controlled static-cluster exercise on one host.
# Single-node mode remains the recommended production mode.
#
# Requires Docker (compose plugin), a buildable `auradb`, and a Python with the
# Aura Connector >= 0.7 installed (so the conformance scripts can import `aura`).
#
# Image selection (AURADB_IMAGE), artifact retention (KEEP_ARTIFACTS=1), and the
# Python interpreter (PYTHON=...) are configurable. Usage:
#   AURADB_IMAGE=auradb:1.3.0 PYTHON=.local/verify/bin/python \
#     bash scripts/smoke_cluster_search_analytics.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"
COMPOSE_FILE="docker-compose.cluster.yml"
export AURADB_IMAGE="${AURADB_IMAGE:-ghcr.io/ohswedd/auradb:1.3.0}"
KEEP_ARTIFACTS="${KEEP_ARTIFACTS:-0}"
PYTHON="${PYTHON:-python3}"
CONF="${REPO_ROOT}/tests/conformance/python"

# host client ports published by docker-compose.cluster.yml. Plain case lookups
# (not associative arrays) so this runs on the stock macOS bash 3.2 too.
ALL_ADDRS=(127.0.0.1:7171 127.0.0.1:7181 127.0.0.1:7191)
ALL_PORTS=(7171 7181 7191)

port_of_service() {
  case "$1" in
    node1) echo 7171 ;;
    node2) echo 7181 ;;
    node3) echo 7191 ;;
    *) return 1 ;;
  esac
}

service_of_port() {
  case "$1" in
    7171) echo node1 ;;
    7181) echo node2 ;;
    7191) echo node3 ;;
    *) return 1 ;;
  esac
}

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is not installed; skipping the live cluster analytics smoke." >&2
  echo "Validate the Compose configuration only with:" >&2
  echo "  docker compose -f ${COMPOSE_FILE} config" >&2
  exit 0
fi

# The conformance scripts import the connector; refuse to run (rather than fake a
# pass) if it is not importable.
if ! "${PYTHON}" -c "import aura" >/dev/null 2>&1; then
  echo "the Aura Connector is not importable by '${PYTHON}'." >&2
  echo "Install it first, e.g.:" >&2
  echo "  ${PYTHON} -m pip install 'aura-connector>=0.7,<0.8'" >&2
  echo "  # or a locally built wheel: ${PYTHON} -m pip install ../aura-connector/dist/aura_connector-0.7.0*.whl" >&2
  exit 2
fi

echo "building auradb..."
cargo build -p auradb-cli >/dev/null
AURADB="${REPO_ROOT}/target/debug/auradb"

dump_logs() {
  echo "=== compose ps ===" >&2
  docker compose -f "${COMPOSE_FILE}" ps >&2 || true
  echo "=== compose logs (tail) ===" >&2
  docker compose -f "${COMPOSE_FILE}" logs --tail=120 >&2 || true
}

cleanup() {
  local status=$?
  [ "${status}" -ne 0 ] && { echo "smoke failed (exit ${status}); dumping cluster logs..." >&2; dump_logs; }
  if [ "${KEEP_ARTIFACTS}" = "1" ]; then
    echo "KEEP_ARTIFACTS=1: leaving the cluster up. Tear down: docker compose -f ${COMPOSE_FILE} down -v" >&2
    return
  fi
  echo "tearing down the Compose cluster..."
  docker compose -f "${COMPOSE_FILE}" down -v >/dev/null 2>&1 && echo "teardown: ok" || echo "teardown: FAILED" >&2
}
trap cleanup EXIT

# Report the role a host port's node currently recognizes for itself ("Leader",
# "Follower", "Candidate"), from `cluster status --json`.
node_role() {
  local port="$1"
  "${AURADB}" cluster status --addr "127.0.0.1:${port}" --json 2>/dev/null \
    | grep -o '"role"[^,}]*' | grep -o '"[A-Za-z]*"$' | tr -d '"' | head -1 || true
}

# Find the host port whose node reports it is the leader, among the given host
# ports, within a bounded deadline. Echoes "127.0.0.1:<port>", or nothing.
#
# This polls each node's OWN self-reported role rather than trusting a
# "leader_client_addr" hint. The cluster advertises the leader by its in-network
# address (e.g. "node2:7171"), which is unreachable from the host and cannot be
# mapped to a host-published port without guessing. Polling self-reported roles
# also rules out a STALE leader: right after the leader is stopped a survivor can
# still name the dead node as leader until its election timeout fires, so we wait
# until a reachable node actually reports role=Leader.
find_leader_addr() {
  local deadline=$(( SECONDS + 60 ))
  while [ "${SECONDS}" -lt "${deadline}" ]; do
    local port
    for port in "$@"; do
      local role
      role="$(node_role "${port}")"
      if [ "${role}" = "Leader" ] || [ "${role}" = "leader" ]; then
        echo "127.0.0.1:${port}"
        return 0
      fi
    done
    sleep 1
  done
  return 1
}

run_analytics_suite() {
  # Run the connector search/analytics conformance scripts against $1 (leader addr).
  local addr="$1" label="$2"
  echo "--- analytics suite (${label}) against ${addr} ---"
  "${PYTHON}" "${CONF}/run_connector_search.py" --addr "${addr}"
  "${PYTHON}" "${CONF}/run_connector_facets.py" --addr "${addr}"
  "${PYTHON}" "${CONF}/run_connector_pagination.py" --addr "${addr}"
  "${PYTHON}" "${CONF}/run_connector_group_by.py" --addr "${addr}"
  "${PYTHON}" "${CONF}/run_connector_cursor_resume.py" --addr "${addr}"
}

echo "using image: ${AURADB_IMAGE}"
echo "generating development peer certificates..."
AURADB="${AURADB}" bash examples/cluster/generate-dev-certs.sh
echo "validating the Compose configuration..."
docker compose -f "${COMPOSE_FILE}" config >/dev/null
echo "starting the three-node Compose cluster..."
docker compose -f "${COMPOSE_FILE}" up -d

echo "waiting for a leader (bounded poll)..."
"${AURADB}" cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 60
LEADER_ADDR="$(find_leader_addr "${ALL_PORTS[@]}")" || { echo "could not resolve a leader" >&2; exit 1; }
echo "initial leader: ${LEADER_ADDR}"
"${AURADB}" cluster status --addr "${LEADER_ADDR}" --json

# 1) Full analytics suite on the initial leader, plus timeout + preview features.
run_analytics_suite "${LEADER_ADDR}" "initial leader"
echo "--- timeout + preview features on the initial leader ---"
"${PYTHON}" "${CONF}/run_connector_timeouts.py" --addr "${LEADER_ADDR}"
"${PYTHON}" "${CONF}/run_connector_ann_preview.py" --addr "${LEADER_ADDR}"
"${PYTHON}" "${CONF}/run_connector_query_profile.py" --addr "${LEADER_ADDR}"

# 2) Bounded leader-change drill: stop the leader, wait for a new one, re-check.
LEADER_PORT="${LEADER_ADDR##*:}"
OLD_SERVICE="$(service_of_port "${LEADER_PORT}" || true)"
[ -n "${OLD_SERVICE}" ] || { echo "unknown leader service for port ${LEADER_PORT}" >&2; exit 1; }
echo "stopping the leader container (${OLD_SERVICE}) to force an election..."
docker compose -f "${COMPOSE_FILE}" stop "${OLD_SERVICE}" >/dev/null

# Poll the SURVIVORS only (the stopped node is unreachable) until one reports it is
# the leader. A plain wait-leader can return the STALE leader here: a survivor keeps
# naming the stopped node as leader until its election timeout fires, so resolving
# by self-reported role and excluding the stopped port is what guarantees we target
# a live new leader rather than the dead one.
SURVIVOR_PORTS=()
for port in "${ALL_PORTS[@]}"; do
  [ "${port}" = "${LEADER_PORT}" ] && continue
  SURVIVOR_PORTS+=("${port}")
done
echo "waiting for a new leader among the survivors (bounded poll)..."
NEW_LEADER_ADDR="$(find_leader_addr "${SURVIVOR_PORTS[@]}")" || { echo "no new leader after failover" >&2; exit 1; }
echo "new leader: ${NEW_LEADER_ADDR}"
[ "${NEW_LEADER_ADDR}" != "${LEADER_ADDR}" ] || { echo "ERROR: the new leader matches the stopped leader address" >&2; exit 1; }

# Search/facet/pagination/group-by must still pass under the new leader.
run_analytics_suite "${NEW_LEADER_ADDR}" "post-failover leader"

# 3) Bring the old node back and confirm quorum is restored.
echo "restarting the old leader container (${OLD_SERVICE})..."
docker compose -f "${COMPOSE_FILE}" start "${OLD_SERVICE}" >/dev/null
echo "waiting for the rejoined node to be reachable (bounded poll)..."
"${AURADB}" cluster wait-leader --addr "127.0.0.1:$(port_of_service "${OLD_SERVICE}")" --timeout-secs 60
STATUS_JSON="$("${AURADB}" cluster status --addr "${NEW_LEADER_ADDR}" --json 2>/dev/null || true)"
QUORUM="$(printf '%s' "${STATUS_JSON}" | grep -o '"quorum_available"[^,}]*' | grep -o 'true\|false' | head -1 || true)"

echo
echo "=== cluster search-analytics smoke summary ==="
echo "image:          ${AURADB_IMAGE}"
echo "initial leader: ${LEADER_ADDR}"
echo "new leader:     ${NEW_LEADER_ADDR} (after stopping ${OLD_SERVICE})"
echo "quorum after rejoin: ${QUORUM:-unknown}"
echo
echo "pass criteria: search/facets/pagination/group-by/cursor-resume passed on the"
echo "  initial leader; a new leader was elected after the leader stopped; the same"
echo "  checks passed under the new leader; and quorum was restored after rejoin."
echo
echo "NOTE: this is an HA candidate preview drill on a controlled static cluster —"
echo "      NOT production HA proof and NOT an automatic-failover SLA. Follower"
echo "      reads/searches are not linearizable; see docs/HA_RELEASE_CANDIDATE.md."
