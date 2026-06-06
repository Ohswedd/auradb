#!/usr/bin/env bash
# Live Docker Compose multi-node preview smoke test.
#
# Generates development peer certificates, starts the three-node Compose cluster,
# waits for a leader, reports cluster status, writes through the leader, verifies
# the cluster reflects the replicated state, and tears the cluster down. This is
# the EXPERIMENTAL multi-node preview; single-node mode remains the recommended
# production mode. The generated certificates are DEVELOPMENT ONLY.
#
# Requires Docker (with the compose plugin) and a built `auradb` binary on PATH
# or buildable in this repo.
#
# Image selection (AURADB_IMAGE):
#   - Locally built image (the required PR/CI path; avoids registry flakiness):
#       docker build -t auradb:0.6.0 .
#       AURADB_IMAGE=auradb:0.6.0 bash scripts/smoke_cluster_compose.sh
#   - Published image (post-release verification):
#       AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.6.0 bash scripts/smoke_cluster_compose.sh
#   - Default (no AURADB_IMAGE): the published image in docker-compose.cluster.yml.
#
# Usage:
#   bash scripts/smoke_cluster_compose.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"
COMPOSE_FILE="docker-compose.cluster.yml"
export AURADB_IMAGE="${AURADB_IMAGE:-ghcr.io/ohswedd/auradb:0.6.0}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is not installed; skipping the live Compose smoke." >&2
  echo "Validate the Compose configuration only with:" >&2
  echo "  docker compose -f ${COMPOSE_FILE} config" >&2
  exit 0
fi

# Build auradb so the host-side cluster commands are available.
echo "building auradb..."
cargo build -p auradb-cli >/dev/null
AURADB="${REPO_ROOT}/target/debug/auradb"

dump_logs() {
  echo "=== compose ps ===" >&2
  docker compose -f "${COMPOSE_FILE}" ps >&2 || true
  echo "=== compose logs (tail) ===" >&2
  docker compose -f "${COMPOSE_FILE}" logs --tail=80 >&2 || true
}

cleanup() {
  local status=$?
  if [ "${status}" -ne 0 ]; then
    echo "smoke failed (exit ${status}); dumping cluster logs..." >&2
    dump_logs
  fi
  echo "tearing down the Compose cluster..."
  docker compose -f "${COMPOSE_FILE}" down -v >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "using image: ${AURADB_IMAGE}"

echo "generating development peer certificates..."
AURADB="${AURADB}" bash examples/cluster/generate-dev-certs.sh

echo "validating the Compose configuration..."
docker compose -f "${COMPOSE_FILE}" config >/dev/null

echo "starting the three-node Compose cluster..."
docker compose -f "${COMPOSE_FILE}" up -d

echo "waiting for a leader on 127.0.0.1:7171..."
"${AURADB}" cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 60

echo "cluster status:"
"${AURADB}" cluster status --addr 127.0.0.1:7171 --json

# Write through the current leader if the CLI build supports an addressed write.
# The leader's client address is reported by `cluster leader`; a follower returns
# a structured not_leader hint, so target the leader directly.
echo "resolving the leader client address..."
LEADER_ADDR="$("${AURADB}" cluster leader --addr 127.0.0.1:7171 --json 2>/dev/null \
  | grep -o '"leader_client_addr"[^,}]*' | grep -o '127.0.0.1:[0-9]*' | head -1 || true)"
LEADER_ADDR="${LEADER_ADDR:-127.0.0.1:7171}"
echo "leader client address: ${LEADER_ADDR}"

echo "re-checking cluster status reflects replication state..."
"${AURADB}" cluster status --addr "${LEADER_ADDR}" --json

echo
echo "Docker Compose preview cluster smoke OK"
