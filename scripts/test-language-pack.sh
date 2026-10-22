#!/usr/bin/env bash
# Networked, disposable CI runtime; never use the operator's profile/service.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$(realpath -m "${1:?output directory required}")"
TEST_ROOT="$(mktemp -d /tmp/ocean-language-pack.XXXXXX)"
TEST_ROOT="$(realpath "$TEST_ROOT")"
cleanup() {
  [[ "$TEST_ROOT" == /tmp/ocean-language-pack.* && "$(realpath "$TEST_ROOT")" == "$TEST_ROOT" && ! -L "$TEST_ROOT" ]] || return 1
  rm -rf -- "${TEST_ROOT:?}"
}
trap cleanup EXIT
mkdir -m 700 "$TEST_ROOT/home"
export HOME="$TEST_ROOT/home" DSH_HOME="$TEST_ROOT/home/.dsh"
export XDG_CONFIG_HOME="$HOME/config" XDG_DATA_HOME="$HOME/data" XDG_STATE_HOME="$HOME/state" XDG_CACHE_HOME="$HOME/cache"
export DSH_OCEAN_DATA_DIR="$TEST_ROOT/runtime"
DSH_NODE_VERSION="$(jq -er '.node.version' "$ROOT/runtime-manifest.json")"
DSH_NODE_SHA256="$(jq -er '.node.archives["linux-x64"].sha256' "$ROOT/runtime-manifest.json")"
DSH_VERSION="$(jq -er '.dsh.version' "$ROOT/runtime-manifest.json")"
DSH_PNPM_VERSION="$(jq -er '.pnpm.version' "$ROOT/runtime-manifest.json")"
export DSH_NODE_VERSION DSH_NODE_SHA256 DSH_VERSION DSH_PNPM_VERSION
unset NODE_OPTIONS NODE_PATH DSH_OCEAN_URL DSH_INSTALL
bash "$ROOT/scripts/bootstrap-runtime.sh"
selected="$(realpath "$DSH_OCEAN_DATA_DIR/current")"
case "$selected" in "$DSH_OCEAN_DATA_DIR"/generations/gen.*) ;; *) exit 1;; esac
export PATH="$selected/runtime/node/bin:$selected/tools/node_modules/.bin:/usr/bin:/bin"
export DSH_INSTALL="$selected/harness"
cd "$ROOT"
npm test --prefix language-pack
python3 -B tests/test_package_language_pack.py
python3 -B scripts/package-language-pack.py --output-dir "$OUT"
python3 -B scripts/package-language-pack.py --verify "$OUT"
# Public checksums/size only, not profiles/logs or the runtime itself.
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    printf '### Optional language pack validation\n\n'
    printf '\x60\x60\x60json\n'
    cat "$OUT/language-pack-artifact.json"
    printf '\x60\x60\x60\n'
  } >> "$GITHUB_STEP_SUMMARY"
fi
