#!/usr/bin/env bash
# Fresh user targets only. Preserve source spelling for no-follow validation.
set -euo pipefail
DIR="${BASH_SOURCE[0]%/*}"
if [[ "$DIR" == "${BASH_SOURCE[0]}" ]]; then DIR=.; fi
case "/$DIR/" in */../*) echo '[install] source parent traversal is unsupported' >&2; exit 1 ;; esac
exec python3 -B "$DIR/install.py" "$@"
