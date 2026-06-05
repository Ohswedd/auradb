#!/usr/bin/env bash
# Generate development peer certificates for the local three-node AuraDB
# multi-node preview cluster (docker-compose.cluster.yml).
#
# DEVELOPMENT ONLY. The generated CA and node certificates are self-signed and
# unencrypted; never use them in production. Single-node mode remains the
# recommended production mode.
#
# It produces, under examples/cluster/certs/ (git-ignored):
#   ca.crt, ca.key                 - a shared local development CA
#   node1.crt/.key, node2.*, node3.*  - per-node certs signed by that CA, each
#                                    with SANs covering its service name plus
#                                    localhost and 127.0.0.1
#
# Each node presents its own certificate; a peer dialing "node2:7172" verifies
# the certificate's SAN against "node2", so per-node SANs are required.
#
# Usage:
#   bash examples/cluster/generate-dev-certs.sh
#
# Then choose a shared peer token and set it identically in every
# examples/cluster/docker/nodeN.toml (peer_auth_token), or export
# AURADB_PEER_TOKEN before running to have this script print a reminder.
set -euo pipefail

# Resolve this script's directory (examples/cluster) and the repo root.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
CERTS_DIR="${SCRIPT_DIR}/certs"

# Find an auradb binary: an explicit AURADB, one on PATH, or build it.
if [[ -n "${AURADB:-}" ]]; then
  :
elif command -v auradb >/dev/null 2>&1; then
  AURADB="auradb"
else
  echo "building auradb (cargo build -p auradb-cli)..." >&2
  (cd "${REPO_ROOT}" && cargo build -p auradb-cli >/dev/null)
  AURADB="${REPO_ROOT}/target/debug/auradb"
fi

mkdir -p "${CERTS_DIR}"

for node in node1 node2 node3; do
  "${AURADB}" cert generate-dev \
    --out-dir "${CERTS_DIR}" \
    --server-name "${node}" \
    --san "${node}" \
    --san localhost \
    --san 127.0.0.1
done

# Restrict private key permissions (best effort).
chmod 600 "${CERTS_DIR}"/*.key 2>/dev/null || true

echo
echo "Development peer certificates written to ${CERTS_DIR}:"
ls -1 "${CERTS_DIR}"
echo
echo "DEVELOPMENT ONLY — do not use these certificates in production."
echo
echo "Next steps:"
echo "  1. Set the SAME peer_auth_token in examples/cluster/docker/node{1,2,3}.toml"
if [[ -n "${AURADB_PEER_TOKEN:-}" ]]; then
  echo "     (suggested token from AURADB_PEER_TOKEN: ${AURADB_PEER_TOKEN})"
fi
echo "  2. docker compose -f docker-compose.cluster.yml up -d"
echo "  3. auradb cluster wait-leader --addr 127.0.0.1:7171 --timeout-secs 60"
echo "  4. auradb cluster status      --addr 127.0.0.1:7171 --json"
