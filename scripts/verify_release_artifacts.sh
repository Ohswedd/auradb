#!/usr/bin/env bash
#
# Verify AuraDB release artifacts for reproducibility and completeness.
#
# Three modes:
#   --dir DIR        Verify a local directory of built artifacts (e.g. `out/`
#                    produced by the Release workflow's collect step). No network.
#   --tag vX.Y.Z     Download the GitHub release assets for the tag (requires the
#                    `gh` CLI) into a temp dir and verify them, and inspect the
#                    release body for the required honesty wording.
#   --self-test      Run a network-free self-test: build synthetic artifact
#                    directories and assert that a good set passes while a
#                    missing archive, a bad checksum, and a wrong-version name
#                    each fail. Exercises the verification path end to end.
#
# Optional:
#   --version X.Y.Z  Expected semantic version (derived from --tag if given).
#
# Checks:
#   1. SHA256SUMS is present and every listed archive verifies.
#   2. Every archive in the directory is listed in SHA256SUMS (no stray asset).
#   3. All current release targets are present as archives.
#   4. Archive names include the version.
#   5. If a host-matching archive exists, `auradb version` runs and prints it.
#   6. (--tag) The GitHub release body carries the required honesty wording.
#
# Usage:
#   scripts/verify_release_artifacts.sh --dir out --version 0.8.1
#   scripts/verify_release_artifacts.sh --tag v0.8.1
#   scripts/verify_release_artifacts.sh --self-test

set -euo pipefail

DIR=""
TAG=""
VERSION=""
SELF_TEST=0

while [ $# -gt 0 ]; do
  case "$1" in
    --dir) DIR="$2"; shift 2 ;;
    --tag) TAG="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --self-test) SELF_TEST=1; shift ;;
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

# Compute the SHA-256 of a file using whichever tool is available.
sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    sha256sum "$1" | awk '{print $1}'
  fi
}

# ---------------------------------------------------------------------------
# Self-test: synthesize artifact directories and assert pass/fail outcomes.
# ---------------------------------------------------------------------------
self_test() {
  local ver="0.8.1"
  local script="$0"
  local base bin_stub
  base="$(mktemp -d)"
  trap 'rm -rf "$base"' RETURN

  # A runnable stub named `auradb` that prints a matching version line, so the
  # host-binary check (#5) is genuinely exercised.
  bin_stub="$base/auradb"
  printf '#!/bin/sh\necho "auradb %s"\n' "$ver" >"$bin_stub"
  chmod +x "$bin_stub"

  # Build a good directory: one archive per target (each carrying the stub) plus
  # a complete SHA256SUMS.
  local good="$base/good"
  mkdir -p "$good"
  local t name
  for t in "${TARGETS[@]}"; do
    name="auradb-${ver}-${t}.tar.gz"
    tar -C "$base" -czf "$good/$name" auradb
  done
  ( cd "$good" && for f in auradb-*.tar.gz; do echo "$(sha256_of "$f")  $f"; done >SHA256SUMS )

  local rc passed=0 failed=0
  run_case() {
    local desc="$1" expect="$2" dir="$3"
    rc=0
    "$script" --dir "$dir" --version "$ver" >/dev/null 2>&1 || rc=$?
    if { [ "$expect" = "pass" ] && [ "$rc" -eq 0 ]; } ||
       { [ "$expect" = "fail" ] && [ "$rc" -ne 0 ]; }; then
      echo "  ok: $desc (expected $expect, rc=$rc)"
      passed=$((passed + 1))
    else
      echo "  FAIL: $desc (expected $expect, rc=$rc)"
      failed=$((failed + 1))
    fi
  }

  echo "self-test: verify_release_artifacts_local_dir_passes"
  run_case "good directory passes" pass "$good"

  echo "self-test: verify_release_artifacts_missing_archive_fails"
  local missing="$base/missing"
  cp -r "$good" "$missing"
  rm -f "$missing/auradb-${ver}-${TARGETS[0]}.tar.gz"
  run_case "missing archive fails" fail "$missing"

  echo "self-test: verify_release_artifacts_bad_checksum_fails"
  local badsum="$base/badsum"
  cp -r "$good" "$badsum"
  # Corrupt the contents of one archive without updating SHA256SUMS.
  echo "tampered" >>"$badsum/auradb-${ver}-${TARGETS[1]}.tar.gz"
  run_case "bad checksum fails" fail "$badsum"

  echo "self-test: verify_release_artifacts_wrong_version_name_fails"
  local wrongver="$base/wrongver"
  mkdir -p "$wrongver"
  for t in "${TARGETS[@]}"; do
    tar -C "$base" -czf "$wrongver/auradb-9.9.9-${t}.tar.gz" auradb
  done
  ( cd "$wrongver" && for f in auradb-*.tar.gz; do echo "$(sha256_of "$f")  $f"; done >SHA256SUMS )
  run_case "wrong version in name fails" fail "$wrongver"

  echo "self-test summary: $passed passed, $failed failed"
  [ "$failed" -eq 0 ]
}

if [ "$SELF_TEST" -eq 1 ]; then
  self_test
  exit $?
fi

if [ -n "$TAG" ]; then
  command -v gh >/dev/null 2>&1 || { echo "gh CLI required for --tag" >&2; exit 2; }
  DIR="$(mktemp -d)"
  echo "downloading release assets for $TAG into $DIR..."
  gh release download "$TAG" --dir "$DIR"
  [ -z "$VERSION" ] && VERSION="${TAG#v}"
fi

if [ -z "$DIR" ]; then
  echo "specify --dir DIR, --tag vX.Y.Z, or --self-test" >&2
  exit 2
fi
[ -d "$DIR" ] || { echo "no such directory: $DIR" >&2; exit 2; }

# Inspect the release body for the required honesty wording (best-effort; only
# when we have a tag and the gh CLI). A scoped release must not silently drop the
# single-node-is-the-recommended-mode / preview-is-not-production-HA framing.
if [ -n "$TAG" ] && command -v gh >/dev/null 2>&1; then
  echo "checking release body honesty wording for $TAG..."
  body="$(gh release view "$TAG" --json body -q .body 2>/dev/null || true)"
  if [ -n "$body" ]; then
    body_lc="$(printf '%s' "$body" | tr '[:upper:]' '[:lower:]')"
    if printf '%s' "$body_lc" | grep -q "single-node" &&
       printf '%s' "$body_lc" | grep -Eq "preview|not production"; then
      echo "  ok: release body carries the scoped-readiness wording"
    else
      echo "FAIL: release body is missing the single-node / preview honesty wording"
      exit 1
    fi
  else
    echo "  note: release body unavailable; skipping wording check"
  fi
fi

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

# 2. Every archive present is accounted for in SHA256SUMS (no stray, unsigned
#    asset slips into a release).
echo "checking SHA256SUMS completeness..."
shopt -s nullglob
for archive in auradb-*.tar.gz auradb-*.zip; do
  if ! grep -q "  $archive\$" SHA256SUMS && ! grep -q " $archive\$" SHA256SUMS; then
    echo "FAIL: archive $archive is not listed in SHA256SUMS"
    fail=1
  fi
done
shopt -u nullglob

# 3 + 4. Every target present, and names carry the version.
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

# 5. Host-matching archive runs and prints the version.
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
