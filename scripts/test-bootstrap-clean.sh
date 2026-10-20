#!/usr/bin/env bash
# Networked integration test. Run in a disposable Linux container without Node.
# Unlike the offline Rust fixtures, this downloads the real runtime and loads
# its native dependencies. Never uses the caller's HOME/profile or port 3080.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if env -i PATH=/usr/bin:/bin /bin/bash -c 'command -v node || command -v nodejs' >/dev/null; then
  echo 'Run this test in a disposable container without system Node.js.' >&2
  exit 77
fi

TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/dsh-ocean-clean.XXXXXX")"
cleanup() {
  case "$TEST_ROOT" in
    "${TMPDIR:-/tmp}/dsh-ocean-clean."*) rm -rf -- "$TEST_ROOT" ;;
  esac
}
trap cleanup EXIT
mkdir -p "$TEST_ROOT/home" "$TEST_ROOT/tmp"

case "$(uname -m)" in
  x86_64) ARCH=linux-x64 ;;
  aarch64) ARCH=linux-arm64 ;;
  *) echo 'unsupported test architecture' >&2; exit 1 ;;
esac
MANIFEST="$ROOT/runtime-manifest.json"
NODE_VERSION="$(jq -er '.node.version' "$MANIFEST")"
NODE_SHA="$(jq -er --arg arch "$ARCH" '.node.archives[$arch].sha256' "$MANIFEST")"
DSH_VERSION="$(jq -er '.dsh.version' "$MANIFEST")"
PNPM_VERSION="$(jq -er '.pnpm.version' "$MANIFEST")"

for attempt in first repeat; do
  printf '\n--- %s clean-bootstrap run ---\n' "$attempt"
  env -i HOME="$TEST_ROOT/home" PATH=/usr/bin:/bin LANG=C.UTF-8 TMPDIR="$TEST_ROOT/tmp" \
    DSH_OCEAN_DATA_DIR="$TEST_ROOT/runtime" DSH_NODE_VERSION="$NODE_VERSION" \
    DSH_NODE_SHA256="$NODE_SHA" DSH_VERSION="$DSH_VERSION" DSH_PNPM_VERSION="$PNPM_VERSION" \
    /bin/bash "$ROOT/scripts/bootstrap-runtime.sh"
done

NODE="$TEST_ROOT/runtime/current/runtime/node/bin/node"
ACTUAL_PNPM="$(env -i HOME="$TEST_ROOT/home" PATH=/usr/bin:/bin \
  "$NODE" "$TEST_ROOT/runtime/current/tools/node_modules/.bin/pnpm" --version)"
[ "$ACTUAL_PNPM" = "$PNPM_VERSION" ] || { echo 'pnpm version mismatch' >&2; exit 1; }
test ! -e "$TEST_ROOT/home/.dsh"
"$NODE" --test "$ROOT/tests/runtime_health.mjs" "$ROOT/tests/service_health.mjs"
echo 'Real clean install, authenticated health, native-module verification and repeat install passed.'
