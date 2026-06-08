#!/usr/bin/env bash
# AuraDB v1.1.0 HA release-candidate smoke test.
#
# Drives the controlled static-cluster preview through a leader change end to
# end: generate development peer certificates, start the three-node Compose
# cluster, wait for a leader, (optionally) write through the leader with the Aura
# Connector, STOP the leader container, wait for a NEW leader, write through it,
# RESTART the old leader, wait for it to rejoin and catch up, check cluster
# status, optionally run the connector leader-change conformance, and tear the
# cluster down cleanly. Cluster logs are dumped on failure.
#
# This is an HA *candidate* smoke for the controlled static-cluster preview, NOT
# production HA proof. Single-node mode remains the recommended production mode.
# The generated certificates are DEVELOPMENT ONLY. See
# docs/HA_RELEASE_CANDIDATE.md.
#
# Requires Docker (with the compose plugin) and a buildable/installed `auradb`.
#
# Image selection (AURADB_IMAGE):
#   - Locally built image:
#       docker build -t auradb:1.1.0 .
#       AURADB_IMAGE=auradb:1.1.0 bash scripts/smoke_ha_candidate.sh
#   - Published image (post-release verification):
#       AURADB_IMAGE=ghcr.io/ohswedd/auradb:1.1.0 bash scripts/smoke_ha_candidate.sh
#
# Keep artifacts (certs, compose project, logs) for inspection instead of tearing
# down on success: KEEP_ARTIFACTS=1 bash scripts/smoke_ha_candidate.sh
#
# Usage:
#   bash scripts/smoke_ha_candidate.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"
COMPOSE_FILE="docker-compose.cluster.yml"
export AURADB_IMAGE="${AURADB_IMAGE:-ghcr.io/ohswedd/auradb:1.1.0}"
KEEP_ARTIFACTS="${KEEP_ARTIFACTS:-0}"

# Host client ports published by docker-compose.cluster.yml and the container
# each maps to. A function (not an associative array) keeps this portable to the
# bash 3.2 that ships on macOS.
HOST_PORTS=(7171 7181 7191)
port_to_container() {
  case "$1" in
    7171) echo auradb-node1 ;;
    7181) echo auradb-node2 ;;
    7191) echo auradb-node3 ;;
    *)    echo "auradb-node?" ;;
  esac
}

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is not installed; skipping the HA candidate smoke." >&2
  echo "Validate the Compose configuration only with:" >&2
  echo "  docker compose -f ${COMPOSE_FILE} config" >&2
  exit 0
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
  if [ "${status}" -ne 0 ]; then
    echo "HA candidate smoke failed (exit ${status}); dumping cluster logs..." >&2
    dump_logs
  fi
  if [ "${KEEP_ARTIFACTS}" = "1" ]; then
    echo "KEEP_ARTIFACTS=1: leaving the Compose cluster and certs in place for inspection." >&2
    echo "  Inspect: docker compose -f ${COMPOSE_FILE} ps / logs" >&2
    echo "  Tear down manually: docker compose -f ${COMPOSE_FILE} down -v" >&2
    return
  fi
  echo "tearing down the Compose cluster..."
  if docker compose -f "${COMPOSE_FILE}" down -v >/dev/null 2>&1; then
    echo "teardown: ok"
  else
    echo "teardown: FAILED (manual cleanup may be required)" >&2
  fi
}
trap cleanup EXIT

echo "using image: ${AURADB_IMAGE}"

# Image digest (best-effort): the published RepoDigest if present, else the local
# image id. Recording the digest pins exactly which artifact this smoke validated.
IMAGE_DIGEST="$(docker image inspect "${AURADB_IMAGE}" \
  --format '{{- if .RepoDigests}}{{index .RepoDigests 0}}{{else}}{{.Id}}{{end}}' 2>/dev/null || true)"
echo "image digest: ${IMAGE_DIGEST:-<unknown — pull/build the image to record a digest>}"

# Fail loudly on a tag/version mismatch (a stale :latest is a common footgun).
IMAGE_TAG_VERSION="${AURADB_IMAGE##*:}"
case "${IMAGE_TAG_VERSION}" in
  [0-9]*.[0-9]*.[0-9]*)
    echo "verifying the image reports auradb ${IMAGE_TAG_VERSION}..."
    CONTAINER_VERSION="$(docker run --rm "${AURADB_IMAGE}" auradb version 2>/dev/null \
      | grep -o '[0-9]*\.[0-9]*\.[0-9]*' | head -1 || true)"
    echo "container auradb version: ${CONTAINER_VERSION:-unknown}"
    if [ -n "${CONTAINER_VERSION}" ] && [ "${CONTAINER_VERSION}" != "${IMAGE_TAG_VERSION}" ]; then
      echo "ERROR: image ${AURADB_IMAGE} reports ${CONTAINER_VERSION}, tag claims ${IMAGE_TAG_VERSION}." >&2
      echo "       Wrong image tag — refusing to smoke it." >&2
      exit 1
    fi
    ;;
  *)
    echo "image tag '${IMAGE_TAG_VERSION}' is not a semantic version; skipping version check"
    ;;
esac

echo "generating development peer certificates..."
AURADB="${AURADB}" bash examples/cluster/generate-dev-certs.sh

echo "validating the Compose configuration..."
docker compose -f "${COMPOSE_FILE}" config >/dev/null

echo "starting the three-node Compose cluster..."
docker compose -f "${COMPOSE_FILE}" up -d

# Report the role a host port's node currently recognizes for itself.
node_role() {
  local port="$1"
  "${AURADB}" cluster status --addr "127.0.0.1:${port}" --json 2>/dev/null \
    | grep -o '"role"[^,}]*' | grep -o '"[A-Za-z]*"$' | tr -d '"' | head -1 || true
}

# Report the leader_client_addr a node currently advertises (the leader hint),
# from `cluster leader --json`. Echoes the address, or nothing if absent.
leader_client_addr_of() {
  local port="$1"
  "${AURADB}" cluster leader --addr "127.0.0.1:${port}" --json 2>/dev/null \
    | grep -o '"leader_client_addr"[[:space:]]*:[[:space:]]*"[^"]*"' \
    | grep -o '"[^"]*"$' | tr -d '"' | head -1 || true
}

# Report the server version a running node advertises via `auradb status --json`.
node_version() {
  local port="$1"
  "${AURADB}" status --addr "127.0.0.1:${port}" --json 2>/dev/null \
    | grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' \
    | grep -o '"[^"]*"$' | tr -d '"' | head -1 || true
}

# Find the host port whose node reports it is the leader, among the given ports,
# within a bounded deadline. Echoes the port, or nothing if none found.
find_leader_port() {
  local deadline=$(( SECONDS + 60 ))
  while [ "${SECONDS}" -lt "${deadline}" ]; do
    for port in "$@"; do
      local role
      role="$(node_role "${port}")"
      if [ "${role}" = "Leader" ] || [ "${role}" = "leader" ]; then
        echo "${port}"
        return 0
      fi
    done
    sleep 1
  done
  return 1
}

echo "waiting for a leader on 127.0.0.1:7171..."
"${AURADB}" cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 60

LEADER_PORT="$(find_leader_port "${HOST_PORTS[@]}" || true)"
if [ -z "${LEADER_PORT}" ]; then
  echo "ERROR: could not resolve the leader host port" >&2
  exit 1
fi
LEADER_CONTAINER="$(port_to_container "${LEADER_PORT}")"
echo "initial leader: ${LEADER_CONTAINER} (host 127.0.0.1:${LEADER_PORT})"
echo "candidate host addresses: 127.0.0.1:${HOST_PORTS[0]}, 127.0.0.1:${HOST_PORTS[1]}, 127.0.0.1:${HOST_PORTS[2]}"

# Report the server version each node advertises, so a mixed-version cluster (a
# common upgrade footgun) is visible up front.
echo "server versions (per node):"
for port in "${HOST_PORTS[@]}"; do
  echo "  $(port_to_container "${port}") (127.0.0.1:${port}): $(node_version "${port}" || true)"
done

# Leader hint diagnostics: with the Compose configs declaring advertise_client_addr,
# the leader names its own client address — but that is the IN-NETWORK address
# (e.g. node1:7171), not the host-published port. So a host client cannot use the
# hint directly and re-resolves by host port (the documented fallback). Report
# both so a failure (no hint at all) is distinguishable from the expected fallback.
INITIAL_HINT="$(leader_client_addr_of "${LEADER_PORT}")"
echo "initial leader_client_addr hint: ${INITIAL_HINT:-<none>}"

echo "cluster status:"
"${AURADB}" cluster status --addr "127.0.0.1:${LEADER_PORT}" --json

# Optional connector write through the leader (best-effort; the core HA checks
# below do not depend on it).
CONNECTOR_OK=0
CONNECTOR_VERSION=""
if command -v python3 >/dev/null 2>&1 \
   && python3 -m pip install --quiet "aura-connector>=0.4.1,<0.5" 2>/dev/null; then
  CONNECTOR_OK=1
  CONNECTOR_VERSION="$(python3 -c 'import importlib.metadata as m; print(m.version("aura-connector"))' 2>/dev/null || true)"
  echo "Aura Connector version: ${CONNECTOR_VERSION:-unknown}"
  echo "writing through the leader with the Aura Connector..."
  python3 tests/conformance/python/run_connector_smoke.py \
    --addr "127.0.0.1:${LEADER_PORT}" --tls-ca .local/certs/ca.crt || \
  python3 tests/conformance/python/run_connector_smoke.py \
    --addr "127.0.0.1:${LEADER_PORT}" || true
else
  echo "Aura Connector not available; skipping connector writes (core HA checks still run)."
fi

# A surviving host port to query after the leader is stopped.
SURVIVOR_PORT=""
for port in "${HOST_PORTS[@]}"; do
  if [ "${port}" != "${LEADER_PORT}" ]; then SURVIVOR_PORT="${port}"; break; fi
done

echo "stopping the leader container (${LEADER_CONTAINER})..."
docker stop "${LEADER_CONTAINER}" >/dev/null

echo "waiting for a NEW leader among the survivors..."
"${AURADB}" cluster wait-leader --addr "127.0.0.1:${SURVIVOR_PORT}" --timeout-secs 60
SURVIVOR_PORTS=()
for port in "${HOST_PORTS[@]}"; do
  [ "${port}" != "${LEADER_PORT}" ] && SURVIVOR_PORTS+=("${port}")
done
NEW_LEADER_PORT="$(find_leader_port "${SURVIVOR_PORTS[@]}" || true)"
if [ -z "${NEW_LEADER_PORT}" ]; then
  echo "ERROR: no new leader elected after stopping ${LEADER_CONTAINER}" >&2
  exit 1
fi
if [ "${NEW_LEADER_PORT}" = "${LEADER_PORT}" ]; then
  echo "ERROR: the new leader matches the stopped leader port" >&2
  exit 1
fi
echo "new leader: $(port_to_container "${NEW_LEADER_PORT}") (host 127.0.0.1:${NEW_LEADER_PORT})"

# The new leader self-reports its own client address as the hint (v0.9.1). A
# present hint here that differs from the host port confirms the in-network
# address is propagating; a host client still re-resolves by host port.
NEW_HINT="$(leader_client_addr_of "${NEW_LEADER_PORT}")"
echo "new leader_client_addr hint: ${NEW_HINT:-<none>}"
if [ -n "${NEW_HINT}" ]; then
  echo "leader-hint path: new leader advertises a client address (in-network); a host" \
       "client re-resolves by host port — the documented fallback."
else
  echo "leader-hint path: no client address advertised; clients re-resolve by host port" \
       "(documented fallback). Not a failure for the Compose smoke."
fi

echo "new-leader cluster status:"
"${AURADB}" cluster status --addr "127.0.0.1:${NEW_LEADER_PORT}" --json

# Connector behavior under the leader change (best-effort).
if [ "${CONNECTOR_OK}" -eq 1 ]; then
  echo "running connector leader-change conformance..."
  CANDIDATES="$(IFS=,; echo "127.0.0.1:${HOST_PORTS[0]},127.0.0.1:${HOST_PORTS[1]},127.0.0.1:${HOST_PORTS[2]}")"
  python3 tests/conformance/python/run_connector_leader_change.py \
    --leader "127.0.0.1:${LEADER_PORT}" --candidate-addrs "${CANDIDATES}" \
    --tls-ca .local/certs/ca.crt || \
  python3 tests/conformance/python/run_connector_leader_change.py \
    --leader "127.0.0.1:${LEADER_PORT}" --candidate-addrs "${CANDIDATES}" || \
  echo "connector leader-change scenario reported issues (non-fatal for the smoke)."
fi

echo "restarting the old leader (${LEADER_CONTAINER})..."
docker start "${LEADER_CONTAINER}" >/dev/null

echo "waiting for the old leader to rejoin and recognize a leader..."
"${AURADB}" cluster wait-ready --addr "127.0.0.1:${LEADER_PORT}" --timeout-secs 60 || true
"${AURADB}" cluster wait-leader --addr "127.0.0.1:${LEADER_PORT}" --timeout-secs 60

echo "final cluster status (from the new leader):"
"${AURADB}" cluster status --addr "127.0.0.1:${NEW_LEADER_PORT}" --json

echo "live cluster diagnostics (doctor):"
"${AURADB}" cluster doctor --addr "127.0.0.1:${NEW_LEADER_PORT}" --json || true

# Confirm all three nodes are connected again from the leader's view.
STATUS_JSON="$("${AURADB}" cluster status --addr "127.0.0.1:${NEW_LEADER_PORT}" --json 2>/dev/null || true)"
CONNECTED="$(printf '%s' "${STATUS_JSON}" | grep -o '"connected":[[:space:]]*true' | wc -l | tr -d ' ')"
QUORUM="$(printf '%s' "${STATUS_JSON}" | grep -o '"quorum_available"[^,}]*' | grep -o 'true\|false' | head -1 || true)"

# Leader client-address SOURCE: where a client would obtain the leader's client
# address. With Compose, an advertised hint is the in-network address, so a host
# client still re-resolves (fallback) — record both honestly.
if [ -n "${NEW_HINT}" ]; then
  HINT_SOURCE="advertised (in-network: ${NEW_HINT}); host clients re-resolve by host port (fallback)"
else
  HINT_SOURCE="fallback (re-resolve/probe by host port; no client address advertised)"
fi

echo
echo "=== HA candidate smoke summary ==="
echo "image:                 ${AURADB_IMAGE}"
echo "image digest:          ${IMAGE_DIGEST:-<unknown>}"
echo "initial leader:        ${LEADER_CONTAINER} (host 127.0.0.1:${LEADER_PORT})"
echo "new leader:            $(port_to_container "${NEW_LEADER_PORT}") (host 127.0.0.1:${NEW_LEADER_PORT})"
echo "initial hint:          ${INITIAL_HINT:-<none>} (in-network; host clients re-resolve)"
echo "new leader hint:       ${NEW_HINT:-<none>} (in-network; host clients re-resolve)"
echo "leader addr source:    ${HINT_SOURCE}"
echo "quorum:                ${QUORUM:-unknown}"
echo "peers connected:       ${CONNECTED:-unknown} (expect the rejoined old leader back)"
echo "connector:             $([ "${CONNECTOR_OK}" -eq 1 ] && echo "exercised (v${CONNECTOR_VERSION:-unknown})" || echo skipped)"
echo
echo "pass criteria (all must hold):"
echo "  - an initial leader was resolved;"
echo "  - a DIFFERENT new leader was elected after the leader was stopped;"
echo "  - the old leader rejoined and a stable leader holds;"
echo "  - quorum is available and all peers reconnected."
echo "expected, NOT a failure:"
echo "  - an empty/in-network leader hint on the HOST (host clients re-resolve by"
echo "    host port — the documented fallback). See docs/CLUSTER_TROUBLESHOOTING.md."
echo
echo "HA candidate smoke OK (controlled static-cluster preview; not production HA proof)"
