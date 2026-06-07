#!/usr/bin/env bash
#
# Verify AuraDB release artifacts for reproducibility and completeness.
#
# Two modes:
#   --dir DIR        Verify a local directory of built artifacts (e.g. `out/`
#                    produced by the Release workflow's collect step).
#   --tag vX.Y.Z     Download the GitHub release assets for the tag (requires the
#                    `gh` CLI) into a temp dir and verify them.
#
# Optional:
#   --version X.Y.Z  Expected semantic version (derived from --tag if given).
#
# Checks:
#   1. SHA256SUMS is present and every listed archive verifies.
#   2. All current release targets are present as archives.
#   3. Archive names include the version.
#   4. If a host-matching archive exists, `auradb version` runs and prints it.
#
# Usage:
#   scripts/verify_release_artifacts.sh --dir out --version 0.8.0
#   scripts/verify_release_artifacts.sh --tag v0.8.0

set -euo pipefail

DIR=""
TAG=""
VERSION=""

while [ $# -gt 0 ]; do
  case "$1" in
    --dir) DIR="$2"; shift 2 ;;
    --tag) TAG="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

TARGETS=(
  "x86_64-unknown-linux-gnu"
  "aarch64-unknown-linux-gnu"
  "x86_64-apple-darwin"
  "aarch64-apple-darwin"
  "x86_64-pc-windows-msvc"
)

if [ -n "$TAG" ]; then
  command -v gh >/dev/null 2>&1 || { echo "gh CLI required for --tag" >&2; exit 2; }
  DIR="$(mktemp -d)"
  echo "downloading release assets for $TAG into $DIR..."
  gh release download "$TAG" --dir "$DIR"
  [ -z "$VERSION" ] && VERSION="${TAG#v}"
fi

if [ -z "$DIR" ]; then
  echo "specify --dir DIR or --tag vX.Y.Z" >&2
  exit 2
fi
[ -d "$DIR" ] || { echo "no such directory: $DIR" >&2; exit 2; }

cd "$DIR"
fail=0

# 1. Checksums.
if [ ! -f SHA256SUMS ]; then
  echo "FAIL: SHA256SUMS missing"
  exit 1
fi
echo "verifying checksums..."
if command -v shasum >/dev/null 2>&1; then
  shasum -a 256 -c SHA256SUMS || fail=1
elif command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c SHA256SUMS || fail=1
else
  echo "FAIL: no shasum/sha256sum available"
  exit 1
fi

# 2 + 3. Every target present, and names carry the version.
echo "checking target coverage..."
for target in "${TARGETS[@]}"; do
  match="$(ls auradb-*"$target".tar.gz auradb-*"$target".zip 2>/dev/null | head -n1 || true)"
  if [ -z "$match" ]; then
    echo "FAIL: no archive for target $target"
    fail=1
    continue
  fi
  if [ -n "$VERSION" ] && ! printf '%s' "$match" | grep -q "$VERSION"; then
    echo "FAIL: archive $match does not carry version $VERSION"
    fail=1
  else
    echo "  ok: $match"
  fi
done

# 4. Host-matching archive runs and prints the version.
host_target=""
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) host_target="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64) host_target="aarch64-unknown-linux-gnu" ;;
  Darwin-x86_64) host_target="x86_64-apple-darwin" ;;
  Darwin-arm64) host_target="aarch64-apple-darwin" ;;
esac
if [ -n "$host_target" ]; then
  archive="$(ls auradb-*"$host_target".tar.gz 2>/dev/null | head -n1 || true)"
  if [ -n "$archive" ]; then
    echo "running auradb version from $archive..."
    workdir="$(mktemp -d)"
    tar -C "$workdir" -xzf "$archive"
    bin="$(find "$workdir" -type f -name auradb | head -n1)"
    if [ -n "$bin" ]; then
      out="$("$bin" version)"
      echo "  -> $out"
      if [ -n "$VERSION" ] && ! printf '%s' "$out" | grep -q "$VERSION"; then
        echo "FAIL: binary reports '$out', expected version $VERSION"
        fail=1
      fi
    fi
    rm -rf "$workdir"
  fi
fi

if [ "$fail" -ne 0 ]; then
  echo "release artifact verification FAILED"
  exit 1
fi
echo "release artifact verification OK"
