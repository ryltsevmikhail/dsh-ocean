#!/usr/bin/env bash
# Explicit URL + independent digest only. Downloaded cache files stay non-executable.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 -B "$ROOT/scripts/appimagetool.py" "$@"
