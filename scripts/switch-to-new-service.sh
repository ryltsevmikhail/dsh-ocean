#!/usr/bin/env bash
# Switch an already-loaded legacy DSH unit to the neutral dsh-web.service.
#
# Run this from a normal terminal, not from inside the DSH web chat: stopping
# the old unit terminates the process serving the current browser session.
#
# The legacy unit name is intentionally not hardcoded. Pass it explicitly:
#   DSH_LEGACY_UNIT=<old-unit> scripts/switch-to-new-service.sh
set -euo pipefail

OLD_UNIT="${DSH_LEGACY_UNIT:-}"
NEW_UNIT="${DSH_NEW_UNIT:-dsh-web.service}"
ASSUME_YES=0

for argument in "$@"; do
  case "$argument" in
    --yes | -y) ASSUME_YES=1 ;;
    --help | -h)
      echo "Usage: DSH_LEGACY_UNIT=<old-unit> scripts/switch-to-new-service.sh [--yes]"
      echo "Stops <old-unit> and starts $NEW_UNIT on 127.0.0.1:3080."
      exit 0
      ;;
    *)
      echo "unknown argument: $argument" >&2
      exit 2
      ;;
  esac
done

if ! systemctl --user cat "$NEW_UNIT" >/dev/null 2>&1; then
  echo "error: $NEW_UNIT is not installed." >&2
  echo "Run the desktop client once so it writes the unit, or reinstall the client." >&2
  exit 1
fi

if systemctl --user is-active --quiet "$NEW_UNIT"; then
  echo "$NEW_UNIT is already active:"
  systemctl --user status "$NEW_UNIT" --no-pager -n 5
  exit 0
fi

if [ -n "$OLD_UNIT" ] && systemctl --user is-active --quiet "$OLD_UNIT"; then
  cat <<'EOF'
[switch] The current DSH web session will disconnect. Sessions stay on disk.
[switch] Run this from a normal terminal, not from inside the DSH chat.
EOF
  echo "[switch] stop: $OLD_UNIT"
  echo "[switch] start: $NEW_UNIT (127.0.0.1:3080)"
  if [ "$ASSUME_YES" != "1" ]; then
    read -r -p "Continue? [y/N] " answer
    case "$answer" in
      y | Y | yes | YES) ;;
      *) echo "cancelled"; exit 0 ;;
    esac
  fi
  systemctl --user stop "$OLD_UNIT"
  systemctl --user daemon-reload
  systemctl --user reset-failed 2>/dev/null || true
  systemctl --user start "$NEW_UNIT"
else
  echo "[switch] no active legacy unit supplied; starting $NEW_UNIT"
  systemctl --user daemon-reload
  systemctl --user start "$NEW_UNIT"
fi

for _ in $(seq 1 40); do
  if systemctl --user is-active --quiet "$NEW_UNIT"; then
    break
  fi
  sleep 0.25
done

if ! systemctl --user is-active --quiet "$NEW_UNIT"; then
  echo "error: $NEW_UNIT failed to start" >&2
  systemctl --user status "$NEW_UNIT" --no-pager -n 0 >&2 || true
  journalctl --user -u "$NEW_UNIT" -n 30 --no-pager -o cat 2>/dev/null |
    sed -E 's/(token=)[A-Za-z0-9._~%-]+/\1***/g' >&2 || true
  exit 1
fi

systemctl --user status "$NEW_UNIT" --no-pager -n 0
echo
echo "[switch] the launch token is in journalctl --user -u $NEW_UNIT"
echo "[switch] restart the desktop client:"
echo "  pkill -x dsh-ocean; dsh-ocean"
