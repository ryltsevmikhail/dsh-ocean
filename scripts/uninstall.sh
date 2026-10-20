#!/usr/bin/env bash
# Remove the DSH desktop client. The shared ~/.dsh profile is never touched.
set -euo pipefail

BIN_DIR="${DSH_OCEAN_BIN_DIR:-$HOME/.local/bin}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}"
PURGE=0

for argument in "$@"; do
  case "$argument" in
    --purge) PURGE=1 ;;
    --help | -h)
      echo "Usage: scripts/uninstall.sh [--purge]"
      echo "  --purge  also stop and remove dsh-web.service, the private runtime, config and state"
      exit 0
      ;;
    *)
      echo "unknown argument: $argument" >&2
      exit 2
      ;;
  esac
done

rm -f "$BIN_DIR/dsh-ocean"
rm -f "$BIN_DIR/dsh-desktop"
rm -f "$DATA_HOME/applications/dev.dsh.ocean.desktop"
rm -f "$DATA_HOME/applications/com.deepseek.harness.desktop"
for size in 32 64 128 256 512; do
  rm -f "$DATA_HOME/icons/hicolor/${size}x${size}/apps/dsh-ocean.png"
  rm -f "$DATA_HOME/icons/hicolor/${size}x${size}/apps/com.deepseek.harness.desktop.png"
done

if [ "$PURGE" = "1" ]; then
  echo "[uninstall] stopping dsh-web.service"
  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now dsh-web.service >/dev/null 2>&1 || true
    rm -f "$CONFIG_HOME/systemd/user/dsh-web.service"
    systemctl --user daemon-reload >/dev/null 2>&1 || true
  fi
  rm -rf "$CONFIG_HOME/dsh-ocean"
  rm -rf "$STATE_HOME/dsh-ocean"
  rm -rf "$DATA_HOME/dsh-ocean"
  echo "[uninstall] removed client config, state and private runtime"
else
  echo "[uninstall] kept ~/.dsh, client config, state and the private runtime"
  echo "[uninstall] enable --purge to remove those too"
fi
