#!/usr/bin/env bash
# Native test harness only: no user session services or mutable XDG state.
set -euo pipefail
if (( $# == 0 )); then
  printf 'usage: %s COMMAND [ARG...]\n' "$0" >&2
  exit 2
fi
root="$(mktemp -d /tmp/dsh-ocean-native-test.XXXXXX)"
root="$(realpath -- "$root")"
cleanup() {
  # Delete only this exact private fixture, never a recomputed/user data path.
  [[ "$(realpath -- "$root")" == "$root" && "$root" == /tmp/dsh-ocean-native-test.* && ! -L "$root" ]] || return 1
  rm -rf -- "${root:?}"
}
trap cleanup EXIT
# libwayland accepts an absolute display socket; preserve desktop connectivity
# while giving helper services a different runtime namespace.
if [[ -n "${WAYLAND_DISPLAY:-}" && "$WAYLAND_DISPLAY" != /* ]]; then
  export WAYLAND_DISPLAY="${XDG_RUNTIME_DIR:?Wayland runtime directory missing}/$WAYLAND_DISPLAY"
fi
export XDG_RUNTIME_DIR="$root/runtime"
export XDG_CONFIG_HOME="$root/config"
export XDG_CACHE_HOME="$root/cache"
export XDG_DATA_HOME="$root/data"
export XDG_STATE_HOME="$root/state"
mkdir -m 700 -- "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME"
export DSH_OCEAN_TEST_PRIVATE_BUS=1
export NO_AT_BRIDGE=1
export GIO_USE_VFS=local
export GSETTINGS_BACKEND=memory
unset AT_SPI_BUS_ADDRESS
# Unlike the stock session config, this has no activatable service directories.
# GTK/WebKit must not launch the user's gvfs/a11y/systemd helpers from this bus.
printf '%s\n' '<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>' > "$root/bus.conf"
dbus-run-session --config-file="$root/bus.conf" -- "$@"
