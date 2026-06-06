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
#       docker build -t auradb:0.6.2 .
#       AURADB_IMAGE=auradb:0.6.2 bash scripts/smoke_cluster_compose.sh
#   - Published image (post-release verification):
#       AURADB_IMAGE=ghcr.io/ohswedd/auradb:0.6.2 bash scripts/smoke_cluster_compose.sh
#   - Default (no AURADB_IMAGE): the published image in docker-compose.cluster.yml.
#
# Usage:
#   bash scripts/smoke_cluster_compose.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"
COMPOSE_FILE="docker-compose.cluster.yml"
export AURADB_IMAGE="${AURADB_IMAGE:-ghcr.io/ohswedd/auradb:0.6.2}"

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
  if docker compose -f "${COMPOSE_FILE}" down -v >/dev/null 2>&1; then
    echo "teardown: ok"
  else
    echo "teardown: FAILED (manual cleanup may be required)" >&2
  fi
}
trap cleanup EXIT

# Node client ports published by docker-compose.cluster.yml.
NODE_PORTS="7171 7172 7173"

echo "using image: ${AURADB_IMAGE}"

# For a registry (multi-arch) image, print the published manifest for the record.
case "${AURADB_IMAGE}" in
  */*:*)
    if docker buildx imagetools inspect "${AURADB_IMAGE}" > /tmp/auradb_manifest 2>/dev/null; then
      echo "image manifest:"
      sed -n '1,24p' /tmp/auradb_manifest
    else
      echo "image manifest: (not available; not a registry image or buildx missing)"
    fi
    ;;
esac

# Validate the image actually reports the expected version and FAIL LOUDLY on a
# tag/version mismatch — a common published-image-smoke footgun is smoking the
# wrong tag (e.g. a stale :latest) and reporting a green run for the wrong build.
IMAGE_TAG_VERSION="${AURADB_IMAGE##*:}"
case "${IMAGE_TAG_VERSION}" in
  [0-9]*.[0-9]*.[0-9]*)
    echo "verifying the image reports auradb ${IMAGE_TAG_VERSION}..."
    CONTAINER_VERSION="$(docker run --rm "${AURADB_IMAGE}" auradb version 2>/dev/null \
      | grep -o '[0-9]*\.[0-9]*\.[0-9]*' | head -1 || true)"
    echo "container auradb version: ${CONTAINER_VERSION:-unknown}"
    if [ -n "${CONTAINER_VERSION}" ] && [ "${CONTAINER_VERSION}" != "${IMAGE_TAG_VERSION}" ]; then
      echo "ERROR: image ${AURADB_IMAGE} reports version ${CONTAINER_VERSION}, but the tag" >&2
      echo "       claims ${IMAGE_TAG_VERSION}. Wrong image tag — refusing to smoke it." >&2
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

echo "running live cluster diagnostics (doctor)..."
"${AURADB}" cluster doctor --addr 127.0.0.1:7171 --json || true

# Structured summary of the published-image smoke run.
STATUS_JSON="$("${AURADB}" cluster status --addr 127.0.0.1:7171 --json 2>/dev/null || true)"
LEADER_ID="$(printf '%s' "${STATUS_JSON}" | grep -o '"leader_id"[^,}]*' | grep -o '"[0-9a-f]*"$' | tr -d '"' | head -1 || true)"
QUORUM="$(printf '%s' "${STATUS_JSON}" | grep -o '"quorum_available"[^,}]*' | grep -o 'true\|false' | head -1 || true)"
PEER_STATES="$(printf '%s' "${STATUS_JSON}" | grep -o '"catch_up_state"[^,}]*' | grep -o '"[a-z_]*"$' | tr -d '"' | paste -sd, - || true)"

echo
echo "=== smoke summary ==="
echo "image:        ${AURADB_IMAGE}"
echo "node ports:   ${NODE_PORTS}"
echo "leader:       ${LEADER_ID:-unknown} (client ${LEADER_ADDR})"
echo "quorum:       ${QUORUM:-unknown}"
echo "peer states:  ${PEER_STATES:-none reported}"
echo
echo "Docker Compose preview cluster smoke OK"
