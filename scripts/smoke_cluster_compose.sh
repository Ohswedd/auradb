#!/usr/bin/env bash
# Live Docker Compose multi-node preview smoke test.
#
# Generates development peer certificates, starts the three-node Compose cluster,
# waits for a leader, reports cluster status, and tears the cluster down. This is
# the EXPERIMENTAL multi-node preview; single-node mode remains the recommended
# production mode. The generated certificates are DEVELOPMENT ONLY.
#
# Requires Docker (with the compose plugin) and a built `auradb` binary on PATH
# or buildable in this repo.
#
# Usage:
#   bash scripts/smoke_cluster_compose.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"
COMPOSE_FILE="docker-compose.cluster.yml"

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

cleanup() {
  echo "tearing down the Compose cluster..."
  docker compose -f "${COMPOSE_FILE}" down -v >/dev/null 2>&1 || true
}
trap cleanup EXIT

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

echo
echo "Docker Compose preview cluster smoke OK"
