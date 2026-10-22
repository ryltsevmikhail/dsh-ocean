#!/usr/bin/env bash
# Private runtime installer. Node, DSH and pnpm form an immutable generation.
# Only `current` is changed on activation; legacy directories are never edited.
# Failed checks restore the old selector. This never restarts a running server.
set -euo pipefail

log() { printf '[bootstrap] %s\n' "$*"; }
die() { printf '[bootstrap] error: %s\n' "$*" >&2; exit 1; }
MODE=install
[ "$#" -le 1 ] || die 'expected at most one argument'
case "${1:-}" in
  --check) MODE=check ;;
  --rollback) MODE=rollback ;;
  --gc) MODE=gc ;;
  --help | -h)
    echo 'Usage: bootstrap-runtime.sh [--check | --rollback | --gc]'
    echo '  --check     report prerequisites and selected runtime'
    echo '  --rollback  health-check and select the previous generation'
    echo '  --gc        remove stale scratch dirs and unreferenced runtime generations'
    echo '              (keeps the selected generation and one rollback hop)'
    exit 0 ;;
  '') ;;
  *) die "unknown argument: $1" ;;
esac

PREFIX="${DSH_OCEAN_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/dsh-ocean}"
NODE_VERSION="${DSH_NODE_VERSION:-26.12.0}"
DSH_VERSION="${DSH_VERSION:-0.2.0-rc.2}"
PNPM_VERSION="${DSH_PNPM_VERSION:-10.34.7}"
case "$(uname -m)" in
  x86_64 | amd64) NODE_ARCH=linux-x64 ;;
  aarch64 | arm64) NODE_ARCH=linux-arm64 ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac
for tool in curl tar gzip sha256sum flock readlink ln mv; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool not found: $tool"
done
mkdir -p -- "$PREFIX"
PREFIX="$(cd -- "$PREFIX" && pwd -P)"
[ "$PREFIX" != / ] || die 'refusing to use the filesystem root as runtime prefix'
[ ! -L "$PREFIX/generations" ] || die 'generations must not be a symlink'
if [ "$MODE" != check ]; then
  if [ "${DSH_OCEAN_INHERITED_LOCK:-}" = 9 ]; then
    # The update worker retains this open file description across installer and
    # service activation. Verify the descriptor, then reassert its flock.
    [ "$(readlink /proc/self/fd/9)" = "$PREFIX/.bootstrap.lock" ] || die 'invalid inherited runtime lock'
  else
    exec 9>"$PREFIX/.bootstrap.lock"
  fi
  flock -n 9 || die "another bootstrap is already running for $PREFIX"
  # The signed transaction owns its generations/scratch until explicit recovery.
  # Recheck under the same prefix lock, including dangling/corrupt journal entries.
  if [ -e "$PREFIX/.runtime-pending.json" ] || [ -L "$PREFIX/.runtime-pending.json" ]; then
    die 'signed runtime recovery is pending; use the native recovery action before maintenance'
  fi
fi

# A forced stop (SIGKILL, app exit) can leave the installer's own transient
# scratch behind. The flock above means no other bootstrap is running, so any
# `.download.*`/`.pointer.*` direct child is stale. Only exact generated names
# qualify; nothing else in the prefix is touched.
sweep_scratch() {
  local removed=0 entry name
  for entry in "$PREFIX"/.download.* "$PREFIX"/.pointer.*; do
    [ -e "$entry" ] || continue
    name="${entry##*/}"
    case "$name" in
      .download.* | .pointer.*) ;;
      *) die "refusing to remove unexpected scratch path: $entry" ;;
    esac
    [ -d "$entry" ] && [ ! -L "$entry" ] || die "refusing to remove non-directory scratch: $entry"
    rm -rf -- "$entry"
    removed=$((removed + 1))
  done
  [ "$removed" -eq 0 ] || log "removed $removed stale scratch director(ies)"
}

# Only relative links to direct children of this prefix are accepted.
generation_root() {
  [[ "$1" =~ ^generations/gen\.[[:alnum:]]+$ ]] || die 'invalid runtime generation selector'
  [ -d "$PREFIX/$1" ] && [ ! -L "$PREFIX/$1" ] || die 'missing runtime generation'
  printf '%s\n' "$PREFIX/$1"
}
if [ "$MODE" != check ]; then
  sweep_scratch
fi
OLD_TARGET=none
ACTIVE_ROOT="$PREFIX"
if [ -L "$PREFIX/current" ]; then
  OLD_TARGET="$(readlink -- "$PREFIX/current")"
  ACTIVE_ROOT="$(generation_root "$OLD_TARGET")"
elif [ -e "$PREFIX/current" ]; then
  die 'current must be a symlink, not a file or directory'
elif [ -x "$PREFIX/runtime/node/bin/node" ] && [ -x "$PREFIX/harness/node_modules/.bin/dsh" ]; then
  OLD_TARGET=legacy
fi

# The managed unit pins a concrete generation in its ExecStart; that generation is
# running (or about to run) and must never be collected.
unit_pinned_generation() {
  local unit="${DSH_OCEAN_UNIT:-$HOME/.config/systemd/user/dsh-web.service}"
  [ -f "$unit" ] && [ ! -L "$unit" ] || return 0
  sed -n 's|.*generations/\(gen\.[[:alnum:]]*\)/.*|generations/\1|p' "$unit" | head -n1
}

if [ "$MODE" = gc ]; then
  [ -d "$PREFIX/generations" ] || { log 'no runtime generations to collect'; exit 0; }
  keep_current=''
  keep_previous=''
  keep_unit="$(unit_pinned_generation)"
  if [ "$OLD_TARGET" != none ] && [ "$OLD_TARGET" != legacy ]; then
    keep_current="$(readlink -- "$PREFIX/current")"
    keep_previous="$(cat -- "$ACTIVE_ROOT/.previous" 2>/dev/null || true)"
  fi
  removed=0
  kept=0
  for entry in "$PREFIX"/generations/gen.*; do
    [ -e "$entry" ] || continue
    [ -d "$entry" ] && [ ! -L "$entry" ] || die "refusing to collect unexpected generation path: $entry"
    name="generations/${entry##*/}"
    if [ "$name" = "$keep_current" ] || [ "$name" = "$keep_previous" ] || [ "$name" = "$keep_unit" ]; then
      kept=$((kept + 1))
      continue
    fi
    rm -rf -- "$entry"
    removed=$((removed + 1))
  done
  log "collected $removed unreferenced generation(s); kept $kept referenced generation(s)"
  exit 0
fi

FREE_KB="$(df -Pk "$PREFIX" | awk 'NR == 2 { print $4 }')"
[ -n "$FREE_KB" ] || die 'could not determine free disk space'
if [ "$MODE" = check ]; then
  log "prefix:   $PREFIX"
  log "selected: $OLD_TARGET"
  log "platform: $NODE_ARCH"
  log "node:     requested $NODE_VERSION"
  log "dsh:      requested $DSH_VERSION"
  log "pnpm:     requested $PNPM_VERSION"
  log "free:     ${FREE_KB} KiB"
  log 'check: ok'
  exit 0
fi

GEN_ROOT=''
DOWNLOAD=''
LINK_TMP=''
SWITCHED=0
COMMITTED=0
remove_owned_temp() {
  case "$1" in
    "$PREFIX/.download."* | "$PREFIX/.pointer."*) rm -rf -- "$1" ;;
    *) die "refusing to remove unexpected temporary path: $1" ;;
  esac
}
select_target() {
  local target="$1"
  if [ "$target" = none ] || [ "$target" = legacy ]; then
    [ ! -e "$PREFIX/current" ] && [ ! -L "$PREFIX/current" ] && return 0
    [ -L "$PREFIX/current" ] || return 1
    rm -- "$PREFIX/current"
  else
    generation_root "$target" >/dev/null || return 1
    LINK_TMP="$(mktemp -d "$PREFIX/.pointer.XXXXXX")"
    ln -s -- "$target" "$LINK_TMP/link" || return 1
    # Both paths are on the same filesystem: rename replaces only the symlink.
    mv -Tf -- "$LINK_TMP/link" "$PREFIX/current" || return 1
    remove_owned_temp "$LINK_TMP"
    LINK_TMP=''
  fi
}
cleanup() {
  local status=$?
  trap - EXIT INT TERM
  set +e
  if [ "$SWITCHED" = 1 ] && [ "$COMMITTED" != 1 ]; then
    log 'activation failed; restoring previous runtime selector'
    if ! select_target "$OLD_TARGET"; then
      log 'ERROR: selector restore failed; retaining candidate for recovery' >&2
      GEN_ROOT=''
      status=1
    fi
  fi
  if [ -n "$GEN_ROOT" ] && [ "$COMMITTED" != 1 ]; then
    # GEN_ROOT was created by this process, is unselected, and is a direct child.
    case "$GEN_ROOT" in
      "$PREFIX/generations/gen."*)
        if [ "$(readlink "$PREFIX/current" 2>/dev/null)" != "generations/${GEN_ROOT##*/}" ]; then
          rm -rf -- "$GEN_ROOT"
        fi ;;
    esac
  fi
  if [ -n "$DOWNLOAD" ]; then remove_owned_temp "$DOWNLOAD"; fi
  if [ -n "$LINK_TMP" ]; then remove_owned_temp "$LINK_TMP"; fi
  exit "$status"
}
trap cleanup EXIT
# A signal that arrives after the new generation was committed must not turn a
# successful install into a reported cancellation: report normal completion.
trap 'if [ "$COMMITTED" = 1 ]; then exit 0; else exit 130; fi' INT
trap 'if [ "$COMMITTED" = 1 ]; then exit 0; else exit 143; fi' TERM

# The helper is embedded by the app; standalone installs use the adjacent file.
health_check() {
  local root="$1" helper
  log 'checking candidate DSH in an isolated profile on a temporary port'
  if [ -n "${DSH_OCEAN_HEALTH_JS:-}" ]; then
    "$root/runtime/node/bin/node" --input-type=module -e "$DSH_OCEAN_HEALTH_JS" "$root"
  else
    # Under `bash -s` (how the app feeds this script) BASH_SOURCE is empty; refuse
    # rather than guessing a helper path in the current directory.
    [ -n "${BASH_SOURCE[0]:-}" ] || die 'health helper is unavailable; run the packaged client instead'
    helper="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/check-runtime.mjs"
    [ -f "$helper" ] || die 'health helper is missing next to the installer'
    "$root/runtime/node/bin/node" "$helper" "$root"
  fi
}
verify_runtime() {
  local root="$1" wanted_node="${2:-}" wanted_dsh="${3:-}" wanted_pnpm="${4:-}"
  local actual_node actual_dsh actual_pnpm
  actual_node="$("$root/runtime/node/bin/node" --version)"
  actual_dsh="$("$root/runtime/node/bin/node" "$root/harness/node_modules/.bin/dsh" --version | tail -n1)"
  actual_pnpm="$("$root/runtime/node/bin/node" "$root/tools/node_modules/.bin/pnpm" --version)"
  [ -n "$actual_node" ] && [ -n "$actual_dsh" ] && [ -n "$actual_pnpm" ] || die 'runtime version probe failed'
  [ -z "$wanted_node" ] || [ "$actual_node" = "v$wanted_node" ] || die 'Node version mismatch'
  [ -z "$wanted_dsh" ] || [ "$wanted_dsh" = latest ] || [ "$actual_dsh" = "$wanted_dsh" ] || die 'DSH version mismatch'
  [ -z "$wanted_pnpm" ] || [ "$actual_pnpm" = "$wanted_pnpm" ] || die 'pnpm version mismatch'
  "$root/runtime/node/bin/node" -e '
    const { createRequire } = require("node:module");
    const load = createRequire(process.argv[1] + "/harness/node_modules/@deepseek-ai/dsh/package.json");
    load("node-pty"); load("koffi");
  ' "$root"
}

if [ "$MODE" = rollback ]; then
  [ "$OLD_TARGET" != none ] && [ "$OLD_TARGET" != legacy ] || die 'no generation selected to roll back'
  IFS= read -r previous <"$ACTIVE_ROOT/.previous"
  case "$previous" in
    none) die 'no previous runtime available' ;;
    legacy) candidate="$PREFIX" ;;
    *) candidate="$(generation_root "$previous")" ;;
  esac
  export PATH="$candidate/runtime/node/bin:$PATH"
  verify_runtime "$candidate"
  health_check "$candidate"
  pinned="$(unit_pinned_generation)"
  if [ -n "$pinned" ] && [ "$pinned" != "$previous" ]; then
    # The unit starts a concrete generation. The caller (the packaged client) is
    # expected to refresh the unit right after this switch; say so loudly because a
    # plain restart would otherwise still run the previous target.
    log "note: the installed unit pins ${pinned##*/}; it must be refreshed before the next restart"
  fi
  SWITCHED=1
  select_target "$previous"
  health_check "$candidate"
  COMMITTED=1
  log "selected previous runtime: $previous; running servers were not restarted"

  exit 0
fi

# Exact metadata plus executable checks: repeats do not modify the active runtime.
expected_versions="$(printf '%s\n' "$NODE_VERSION" "$DSH_VERSION" "$PNPM_VERSION" "${DSH_NODE_SHA256:-upstream}")"
if [ "$DSH_VERSION" != latest ] && [ -f "$ACTIVE_ROOT/.versions" ] && \
   [ "$(<"$ACTIVE_ROOT/.versions")" = "$expected_versions" ]; then
  export PATH="$ACTIVE_ROOT/runtime/node/bin:$PATH"
  verify_runtime "$ACTIVE_ROOT" "$NODE_VERSION" "$DSH_VERSION" "$PNPM_VERSION"
  health_check "$ACTIVE_ROOT"
  log "runtime already verified at $ACTIVE_ROOT"
  exit 0
fi
[ "$FREE_KB" -ge 1500000 ] || die "at least 1.5 GiB free is required under $PREFIX"
mkdir -p -- "$PREFIX/generations"
GEN_ROOT="$(mktemp -d "$PREFIX/generations/gen.XXXXXX")"
TARGET="generations/${GEN_ROOT##*/}"
mkdir -p "$GEN_ROOT/runtime/node" "$GEN_ROOT/harness" "$GEN_ROOT/tools"
printf '%s\n' "$OLD_TARGET" >"$GEN_ROOT/.previous"
printf '%s\n' "$expected_versions" >"$GEN_ROOT/.versions"

DOWNLOAD="$(mktemp -d "$PREFIX/.download.XXXXXX")"
archive="node-v${NODE_VERSION}-${NODE_ARCH}.tar.gz"
log "downloading Node.js $NODE_VERSION ($NODE_ARCH)"
curl -fL --retry 3 --retry-delay 1 -o "$DOWNLOAD/$archive" "https://nodejs.org/dist/v${NODE_VERSION}/${archive}"
expected="${DSH_NODE_SHA256:-}"
if [ -z "$expected" ]; then
  curl -fL --retry 3 --retry-delay 1 -o "$DOWNLOAD/SHASUMS256.txt" "https://nodejs.org/dist/v${NODE_VERSION}/SHASUMS256.txt"
  expected="$(grep " ${archive}$" "$DOWNLOAD/SHASUMS256.txt" | awk '{print $1}' | head -n1)"
fi
[ -n "$expected" ] || die "no checksum for $archive"
actual="$(sha256sum "$DOWNLOAD/$archive" | awk '{print $1}')"
[ "$expected" = "$actual" ] || die 'Node.js checksum mismatch'
tar -xzf "$DOWNLOAD/$archive" -C "$GEN_ROOT/runtime/node" --strip-components=1
[ "$("$GEN_ROOT/runtime/node/bin/node" --version)" = "v$NODE_VERSION" ] || die 'staged Node version mismatch'
remove_owned_temp "$DOWNLOAD"
DOWNLOAD=''

# npm and lifecycle scripts always use the candidate interpreter, never system Node.
export PATH="$GEN_ROOT/runtime/node/bin:$PATH"
NPM="$GEN_ROOT/runtime/node/bin/npm"
log "installing candidate @deepseek-ai/dsh@$DSH_VERSION"
"$NPM" install --prefix "$GEN_ROOT/harness" --no-audit --no-fund "@deepseek-ai/dsh@$DSH_VERSION"
if (cd "$GEN_ROOT/harness" && "$NPM" install-scripts ls >/dev/null 2>&1); then
  (cd "$GEN_ROOT/harness" && "$NPM" install-scripts approve --all && "$NPM" rebuild)
fi
"$NPM" install --prefix "$GEN_ROOT/tools" --no-audit --no-fund "pnpm@$PNPM_VERSION"
verify_runtime "$GEN_ROOT" "$NODE_VERSION" "$DSH_VERSION" "$PNPM_VERSION"
health_check "$GEN_ROOT"
SWITCHED=1
select_target "$TARGET"
# Verify the selected path as well; any failure here restores OLD_TARGET in EXIT.
SELECTED="$(generation_root "$(readlink "$PREFIX/current")")"
verify_runtime "$SELECTED" "$NODE_VERSION" "$DSH_VERSION" "$PNPM_VERSION"
health_check "$SELECTED"
COMMITTED=1
log "ready: $PREFIX/current/harness/node_modules/.bin/dsh"
log "selected $TARGET; previous $OLD_TARGET retained; running servers were not restarted"
