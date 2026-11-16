#!/usr/bin/env bash
# Production-protocol enrollment; passwords/PATs stay in private files or child environment.
set -euo pipefail
exec python3 -I "$(dirname -- "$(readlink -f -- "${BASH_SOURCE[0]}")")/devices.py" "$@"
