#!/usr/bin/env bash
# Build the portable native tar from Cargo's actual reported executable.
# No installation, no deletion of old staging/output, and no implicit overwrite.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 -B "$ROOT/scripts/package-tar.py" "$@"
