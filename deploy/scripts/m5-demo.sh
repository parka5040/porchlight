#!/usr/bin/env bash
# S6a server gate: --sandbox uses isolated real-TLS Rust fixtures; --stack checks a provisioned dev stack.
# --stack also checks two recipient devices plus another member, then chains the S6b stream gate.
# Stack config and all diagnostics stay under var/. Never print credential material or arbitrary errors.
set -euo pipefail
umask 022
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
case ${1:-} in
  --client)
    shift
    exec bash deploy/scripts/m5-client-demo.sh "$@"
    ;;
  --sandbox)
    . scripts/env.sh
    export CARGO_NET_OFFLINE=true
    mkdir -p var/m5-demo
    if cargo test --manifest-path server/Cargo.toml -p porchlight-api --features testing --locked --test live -- notifications:: stream:: >var/m5-demo/server-tests.log 2>&1; then
      printf 'PASS: M5 server routing/coalescing/quiet-hours/inbox and TLS authorization fixtures\n'
    else
      printf 'FAIL: M5 server fixtures (private diagnostics in var/m5-demo)\n' >&2
      exit 1
    fi
    ;;
  --stack)
    . scripts/env.sh
    python3 deploy/scripts/m5-notifications-demo.py "${2:-var/m5-demo/config.json}"
    ;;
  *) printf 'FAIL: use m5-demo.sh --sandbox | --stack private-config | --client --stack private-config --serial disposable-emulator [--keep]\n' >&2; exit 2 ;;
esac
