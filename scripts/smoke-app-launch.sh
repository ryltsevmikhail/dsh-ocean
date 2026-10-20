#!/usr/bin/env bash
# Launch smoke: prove the real binary starts, stays alive and does not panic.
#
# Test-only harness. Call it through scripts/test-native.sh (private XDG dirs and a
# private session bus) and under a display, e.g.:
#   xvfb-run -a bash scripts/smoke-app-launch.sh target/debug/dsh-ocean
#
# The app must NOT talk to a real Harness: the origin is set to an unroutable
# documentation-range address and DSH_HOME points into the private fixture.
set -euo pipefail
binary="${1:?usage: smoke-app-launch.sh PATH/TO/dsh-ocean}"
seconds="${SMOKE_SECONDS:-12}"
[[ -x "$binary" ]] || { printf 'not an executable: %s\n' "$binary" >&2; exit 2; }
[[ "${XDG_RUNTIME_DIR:?run through scripts/test-native.sh}" == /tmp/dsh-ocean-native-test.* ]] || {
  printf 'refusing to run outside the private native harness\n' >&2
  exit 2
}
root="$(realpath -- "${XDG_RUNTIME_DIR:?}")"
[[ "$root" == /tmp/dsh-ocean-native-test.* && ! -L "$root" ]] || {
  printf 'unexpected private root: %s\n' "$root" >&2
  exit 2
}
export HOME="$root/home"
export DSH_HOME="$root/home/.dsh"
export DSH_OCEAN_URL=http://127.0.0.1:9/
# Software rendering only: the CI display has no GPU and the app must not depend on one.
export WEBKIT_DISABLE_DMABUF_RENDERER=1
export GDK_BACKEND=x11
unset WAYLAND_DISPLAY
export DSH_OCEAN_DATA_DIR="$root/data/dsh-ocean"
mkdir -p -- "$HOME" "$DSH_HOME" "$DSH_OCEAN_DATA_DIR"
chmod 700 -- "$HOME" "$DSH_HOME" "$DSH_OCEAN_DATA_DIR"
log="$root/app-launch.log"

# Run the app in the background so the display can be inspected while it lives.
"$binary" >"$log" 2>&1 &
app_pid=$!
idle_pid=''
stop_app() {
  if [[ -n "$idle_pid" ]]; then
    kill -TERM "$idle_pid" 2>/dev/null || true
    wait "$idle_pid" 2>/dev/null || true
    idle_pid=''
  fi
  [[ -n "$app_pid" ]] || return 0
  if kill -0 "$app_pid" 2>/dev/null; then
    kill -TERM "$app_pid" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      kill -0 "$app_pid" 2>/dev/null || break
      sleep 0.2
    done
    kill -KILL "$app_pid" 2>/dev/null || true
  fi
  wait "$app_pid" 2>/dev/null || true
}
trap stop_app EXIT

sleep "$seconds"
if ! kill -0 "$app_pid" 2>/dev/null; then
  printf 'launch smoke failed: the app exited before %ss\n' "$seconds" >&2
  sed -n '1,120p' "$log" >&2
  exit 1
fi
if [[ -s "$log" ]] && grep -qiE 'panicked|thread .main. panicked' "$log"; then
  printf 'launch smoke failed: the app panicked\n' >&2
  sed -n '1,120p' "$log" >&2
  exit 1
fi

# A real window is the point of the smoke: without one the app is running but
# unusable. Skipped only when the inspection tool is unavailable.
window_count=0
if [[ "${SMOKE_EXPECT_WINDOW:-1}" == 1 ]]; then
  if command -v xwininfo >/dev/null 2>&1; then
    window_count="$(xwininfo -root -tree 2>/dev/null | grep -ci 'dsh ocean' || true)"
    if [[ "$window_count" != 1 ]]; then
      printf 'launch smoke failed: expected one DSH Ocean window, found %s\n' "$window_count" >&2
      xwininfo -root -tree 2>/dev/null | sed -n '1,40p' >&2
      exit 1
    fi
  else
    printf 'launch smoke note: xwininfo is unavailable, the window is not asserted\n'
  fi
fi

# A second launch must hand over to the running instance and exit promptly
# instead of starting a second desktop session.
if [[ "${SMOKE_CHECK_SINGLE_INSTANCE:-1}" == 1 ]]; then
  second_log="$root/second-launch.log"
  started=true
  timeout --kill-after=5s 10 "$binary" >"$second_log" 2>&1 || started=false
  if [[ "$started" != true ]]; then
    printf 'launch smoke failed: a second launch did not exit by itself\n' >&2
    sed -n '1,60p' "$second_log" >&2
    exit 1
  fi
  if ! kill -0 "$app_pid" 2>/dev/null; then
    printf 'launch smoke failed: the first instance exited after a second launch\n' >&2
    exit 1
  fi
  if [[ "$window_count" == 1 ]]; then
    after="$(xwininfo -root -tree 2>/dev/null | grep -ci 'dsh ocean' || true)"
    if [[ "$after" != 1 ]]; then
      printf 'launch smoke failed: a second launch opened another window (%s)\n' "$after" >&2
      exit 1
    fi
  fi
  printf 'launch smoke ok: a second launch exited and left one window; one instance is live after %ss\n' "$seconds"
else
  printf 'launch smoke ok: the app stayed alive for %ss\n' "$seconds"
fi

# Use the actual window-close protocol, not XDestroyWindow or a process signal:
# this must exercise the application's CloseRequested handler.
if [[ "${SMOKE_CHECK_CLOSE_PREFERENCE:-1}" == 1 ]]; then
  command -v xwininfo >/dev/null || { printf 'close smoke requires xwininfo\n' >&2; exit 1; }
  script_dir="$(dirname -- "$(realpath -- "${BASH_SOURCE[0]}")")"
  window_id="$(xwininfo -root -tree 2>/dev/null | awk '/"DSH Ocean"/ && !id { id=$1 } END { print id }')"
  [[ "$window_id" =~ ^0x[0-9a-fA-F]+$ ]] || { printf 'close smoke found no primary window\n' >&2; exit 1; }
  wait_map_state() {
    local expected="$1"
    for _ in {1..50}; do
      kill -0 "$app_pid" 2>/dev/null || return 1
      if xwininfo -id "$window_id" 2>/dev/null | grep -q "Map State: $expected"; then
        return 0
      fi
      sleep 0.1
    done
    return 1
  }
  DSH_OCEAN_TEST_X11=1 python3 "$script_dir/test-x11-close.py" "$window_id"
  if ! wait_map_state IsUnMapped; then
    printf 'close smoke failed: default close did not hide a live window\n' >&2
    exit 1
  fi

  # Write only inside the harness, never to a caller's real XDG configuration.
  fixture_root="$(dirname -- "$root")"
  config_home="$(realpath -m -- "${XDG_CONFIG_HOME:?}")"
  [[ "$(basename -- "$root")" == runtime && "$config_home" == "$fixture_root/"* ]] || {
    printf 'close smoke refused a non-private configuration directory\n' >&2
    exit 2
  }
  mkdir -p -- "$config_home/dsh-ocean"
  printf '%s\n' '{"closeToTray":false}' > "$config_home/dsh-ocean/config.json"
  # A peer that connects but never writes used to block every later activation.
  python3 - "$root/dsh-ocean.sock" "$root/idle-client.ready" <<'PY' &
import pathlib
import socket
import sys
with socket.socket(socket.AF_UNIX) as client:
    client.connect(sys.argv[1])
    pathlib.Path(sys.argv[2]).write_text("ready\n")
    client.recv(1)
PY
  idle_pid=$!
  for _ in {1..50}; do
    [[ -f "$root/idle-client.ready" ]] && break
    kill -0 "$idle_pid" 2>/dev/null || break
    sleep 0.1
  done
  [[ -f "$root/idle-client.ready" ]] || { printf 'activation smoke failed: idle peer did not connect\n' >&2; exit 1; }
  timeout --kill-after=5s 10 "$binary" > "$root/restore-window.log" 2>&1
  if ! wait_map_state IsViewable; then
    printf 'close smoke failed: single-instance activation did not restore the window with an idle peer connected\n' >&2
    exit 1
  fi
  wait "$idle_pid"
  idle_pid=''
  DSH_OCEAN_TEST_X11=1 python3 "$script_dir/test-x11-close.py" "$window_id"
  for _ in {1..50}; do
    kill -0 "$app_pid" 2>/dev/null || break
    sleep 0.1
  done
  if kill -0 "$app_pid" 2>/dev/null; then
    printf 'close smoke failed: closeToTray=false did not quit the app\n' >&2
    exit 1
  fi
  if ! wait "$app_pid"; then
    printf 'close smoke failed: preference-driven shutdown returned an error\n' >&2
    exit 1
  fi
  app_pid=''
  if grep -qi 'panicked' "$log"; then
    printf 'close smoke failed: the app panicked during close handling\n' >&2
    exit 1
  fi
  printf 'close smoke ok: default close hid, activation restored, closeToTray=false exited cleanly\n'
fi
