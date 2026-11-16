#!/usr/bin/env bash
# Host-only collector; implementation stays stdlib-only and fixed-query.
set -euo pipefail
exec python3 -I "$(dirname -- "$(readlink -f -- "${BASH_SOURCE[0]}")")/observe.py" "$@"
