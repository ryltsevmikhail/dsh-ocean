#!/usr/bin/env bash
# Build a fresh Debian archive without installing it or deleting old staging.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 -B "$ROOT/scripts/package-deb.py" "$@"
