#!/usr/bin/env bash
# Loopback-only HTTP mirror; Docker and minisign are offline stand-ins.
set -euo pipefail
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
python3 -I -B "$root/deploy/tests/github-upgrade.py"
