#!/usr/bin/env bash
set -euo pipefail
umask 077
export PYTHONDONTWRITEBYTECODE=1
base=$(cd "$(dirname "$0")" && pwd)
# No private recovery key is ever mounted into this job.
exec python3 "$base/backup.py" "${1:-daemon}"
