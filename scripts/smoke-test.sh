#!/usr/bin/env bash
# Local smoke test. Requires an installed DSH for --runtime/--print-unit.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# GUI-launched shells may not inherit rustup's PATH.
if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi
command -v cargo >/dev/null 2>&1 || { echo "cargo is required" >&2; exit 1; }

SMOKE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/dsh-ocean-smoke.XXXXXX")"
cleanup() {
  case "$SMOKE_TMP" in
    "${TMPDIR:-/tmp}/dsh-ocean-smoke."*) rm -rf -- "$SMOKE_TMP" ;;
  esac
}
trap cleanup EXIT

cargo test --locked --quiet
cargo build --locked --quiet
./target/debug/dsh-ocean --version
./target/debug/dsh-ocean --runtime >"$SMOKE_TMP/runtime.json"
jq -e '.ready == true' "$SMOKE_TMP/runtime.json" >/dev/null
./target/debug/dsh-ocean --print-unit >"$SMOKE_TMP/dsh-web.service"
systemd-analyze verify "$SMOKE_TMP/dsh-web.service"
DSH_OCEAN_DATA_DIR="$SMOKE_TMP/bootstrap" bash scripts/bootstrap-runtime.sh --check
echo "smoke test passed"
