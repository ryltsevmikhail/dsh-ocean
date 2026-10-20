#!/usr/bin/env bash
# Fresh inputs and explicit pinned tool; no reused staging or implicit overwrite.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 -B "$ROOT/scripts/package-appimage.py" "$@"
