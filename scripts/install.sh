#!/usr/bin/env bash
# Install the DSH desktop client for the current user.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${DSH_OCEAN_BIN_DIR:-$HOME/.local/bin}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
APP_DIR="$DATA_HOME/applications"
ICON_ROOT="$DATA_HOME/icons/hicolor"

# Desktop Exec has two escaping layers: argv quoting, then desktop string
# escaping. Never interpolate an install path into shell or sed source.
desktop_exec() {
  local value="$1" result='"' char index
  for ((index = 0; index < ${#value}; index++)); do
    char="${value:index:1}"
    case "$char" in
      '%') result+='%%' ;;
      '\') result+='\\\\' ;;
      '"' | '$' | '`') result+='\\'; result+="$char" ;;
      *) result+="$char" ;;
    esac
  done
  printf '%s"' "$result"
}

BUILD=1
for argument in "$@"; do
  case "$argument" in
    --no-build) BUILD=0 ;;
    --help | -h)
      echo "Usage: scripts/install.sh [--no-build]"
      exit 0
      ;;
    *)
      echo "unknown argument: $argument" >&2
      exit 2
      ;;
  esac
done

# Control characters cannot be represented literally in desktop entry values.
if [[ "$BIN_DIR" != /* || "$BIN_DIR" =~ [[:cntrl:]] ]]; then
  echo "[install] binary directory must be absolute and contain no control characters" >&2
  exit 2
fi

if [ "$BUILD" = "1" ] || [ ! -x "$ROOT/target/release/dsh-ocean" ]; then
  echo "[install] building release binary"
  (cd "$ROOT" && cargo build --release --locked)
fi

BIN="$ROOT/target/release/dsh-ocean"
[ -x "$BIN" ] || { echo "missing $BIN" >&2; exit 1; }

# Validate the complete reviewed notice payload before replacing any target.
LICENSE_FILES=()
declare -A LICENSE_SEEN=()
RELEASE_TOOLCHAIN_FILES=(
  docs/toolchain/rust-1.90.0/index.json
  docs/toolchain/rust-1.90.0/COPYRIGHT-library.html
  docs/toolchain/rust-1.90.0/COPYRIGHT.html
  docs/toolchain/rust-1.90.0/licenses/Apache-2.0.txt
  docs/toolchain/rust-1.90.0/licenses/BSD-2-Clause.txt
  docs/toolchain/rust-1.90.0/licenses/CC-BY-SA-4.0.txt
  docs/toolchain/rust-1.90.0/licenses/GCC-exception-3.1.txt
  docs/toolchain/rust-1.90.0/licenses/GPL-2.0-only.txt
  docs/toolchain/rust-1.90.0/licenses/GPL-3.0-or-later.txt
  docs/toolchain/rust-1.90.0/licenses/ISC.txt
  docs/toolchain/rust-1.90.0/licenses/LLVM-exception.txt
  docs/toolchain/rust-1.90.0/licenses/MIT.txt
  docs/toolchain/rust-1.90.0/licenses/NCSA.txt
  docs/toolchain/rust-1.90.0/licenses/OFL-1.1.txt
  docs/toolchain/rust-1.90.0/licenses/Unicode-3.0.txt
  docs/toolchain/rust-1.90.0/LICENSE-MIT
  docs/toolchain/rust-1.90.0/LICENSE-APACHE
  docs/toolchain/rust-1.90.0/LICENSE-COMPILER-BUILTINS
  docs/toolchain/rust-1.90.0/LICENSE-COMPILER-BUILTINS-LIBM
  docs/toolchain/rust-1.90.0/SOURCE-COMPILER-BUILTINS-Cargo.toml
  docs/toolchain/rust-1.90.0/SOURCE-UNWIND-Cargo.toml
  docs/toolchain/rust-1.90.0/SOURCE-UNWIND-lib.rs
  docs/toolchain/rust-1.90.0/COPYRIGHT-guide.txt
  docs/toolchain/rust-1.90.0/rust-src-1.90.0.tar.xz
  docs/toolchain/rust-1.90.0/COMPILER-RT-CREDITS.TXT
  docs/toolchain/rust-1.90.0/GNU-glibc-source.dsc
  docs/toolchain/rust-1.90.0/GNU-glibc-copyright
  docs/toolchain/rust-1.90.0/GNU-gcc-13-source.dsc
  docs/toolchain/rust-1.90.0/GNU-gcc-13-copyright
  docs/toolchain/rust-1.90.0/GNU-glibc-x86_64-start.S
  docs/toolchain/rust-1.90.0/GNU-glibc-x86_64-crti.S
  docs/toolchain/rust-1.90.0/GNU-glibc-x86_64-crtn.S
  docs/toolchain/rust-1.90.0/GNU-glibc-aarch64-start.S
  docs/toolchain/rust-1.90.0/GNU-glibc-aarch64-crti.S
  docs/toolchain/rust-1.90.0/GNU-glibc-aarch64-crtn.S
  docs/toolchain/rust-1.90.0/GNU-glibc-COPYING.LIB
  docs/toolchain/rust-1.90.0/GNU-gcc-crtstuff.c
  docs/toolchain/rust-1.90.0/COMPILER-RT-AARCH64-source.tar.gz
  docs/toolchain/rust-1.90.0/COMPILER-RT-AARCH64-index.json
)
declare -A RELEASE_TOOLCHAIN_ALLOWED=()
for notice in "${RELEASE_TOOLCHAIN_FILES[@]}"; do RELEASE_TOOLCHAIN_ALLOWED[$notice]=1; done
license_pattern='^docs/third-party/sources/[a-z0-9][a-z0-9_-]*-[0-9][0-9A-Za-z._+-]*\.crate$'
[ -f "$ROOT/packaging/license-inputs.txt" ] && [ ! -L "$ROOT/packaging/license-inputs.txt" ] || { echo "missing regular license payload manifest" >&2; exit 1; }
while IFS= read -r relative || [ -n "$relative" ]; do
  [[ -z "$relative" || "$relative" == \#* ]] && continue
  case "$relative" in
    LICENSE|NOTICE|THIRD_PARTY_NOTICES.txt|locales/dsh/LICENSE|docs/third-party/inventory.json|docs/BINARY_LICENSE_AUDIT.md) ;;
    *) [[ -n "${RELEASE_TOOLCHAIN_ALLOWED[$relative]+yes}" || "$relative" =~ $license_pattern ]] || { echo "unsupported license payload path" >&2; exit 1; } ;;
  esac
  [[ -z "${LICENSE_SEEN[$relative]+yes}" ]] || { echo "duplicate license payload path" >&2; exit 1; }
  LICENSE_SEEN[$relative]=1
  LICENSE_FILES+=("$relative")
  IFS=/ read -r -a components <<< "$relative"
  current="$ROOT"
  for component in "${components[@]}"; do
    current="$current/$component"
    [ ! -L "$current" ] || { echo "symlink in license payload" >&2; exit 1; }
  done
  [ -f "$current" ] || { echo "missing license payload" >&2; exit 1; }
done < "$ROOT/packaging/license-inputs.txt"
REQUIRED_LICENSE_FILES=(LICENSE NOTICE THIRD_PARTY_NOTICES.txt locales/dsh/LICENSE docs/third-party/inventory.json docs/BINARY_LICENSE_AUDIT.md
  docs/third-party/sources/cssparser-0.37.0.crate docs/third-party/sources/cssparser-macros-0.7.1.crate
  docs/third-party/sources/dtoa-short-0.3.5.crate docs/third-party/sources/option-ext-0.2.0.crate
  docs/third-party/sources/selectors-0.38.0.crate)
REQUIRED_LICENSE_FILES+=("${RELEASE_TOOLCHAIN_FILES[@]}")
for required in "${REQUIRED_LICENSE_FILES[@]}"; do
  [[ -n "${LICENSE_SEEN[$required]+yes}" ]] || { echo "required license payload missing" >&2; exit 1; }
done
[ "${#LICENSE_FILES[@]}" -eq "${#REQUIRED_LICENSE_FILES[@]}" ] || { echo "unexpected license payload" >&2; exit 1; }

echo "[install] binary -> $BIN_DIR/dsh-ocean"
install -Dm755 "$BIN" "$BIN_DIR/dsh-ocean"

for size in 32 64 128 256 512; do
  case "$size" in
    32) source="$ROOT/icons/32x32.png" ;;
    64) source="$ROOT/icons/64x64.png" ;;
    128) source="$ROOT/icons/128x128.png" ;;
    256) source="$ROOT/icons/128x128@2x.png" ;;
    512) source="$ROOT/icons/icon.png" ;;
  esac
  install -Dm644 "$source" "$ICON_ROOT/${size}x${size}/apps/dsh-ocean.png"
done

install -Dm644 "$ROOT/packaging/dsh-ocean.desktop" "$APP_DIR/dev.dsh.ocean.desktop"
{
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      Exec=*) printf 'Exec=%s\n' "$(desktop_exec "$BIN_DIR/dsh-ocean")" ;;
      *) printf '%s\n' "$line" ;;
    esac
  done < "$ROOT/packaging/dsh-ocean.desktop"
} > "$APP_DIR/dev.dsh.ocean.desktop"

for relative in "${LICENSE_FILES[@]}"; do
  install -Dm644 "$ROOT/$relative" "$DATA_HOME/doc/dsh-ocean/$relative"
done
install -Dm644 "$ROOT/packaging/license-inputs.txt" "$DATA_HOME/doc/dsh-ocean/packaging/license-inputs.txt"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$ICON_ROOT" >/dev/null 2>&1 || true
fi

cat <<EOF
[install] done

Launch "DSH Ocean" from your app launcher, or run:
  $BIN_DIR/dsh-ocean

First run: the client installs a private Node.js + DSH runtime if needed and
creates an enabled ~/.config/systemd/user/dsh-web.service.
EOF
