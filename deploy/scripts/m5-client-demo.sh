#!/usr/bin/env bash
# Controller only: disposable emulator + enrolled disposable desktop profile + local dev stack.
set -euo pipefail
umask 022
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
. scripts/env.sh
. scripts/emulator-common.sh
mkdir -p var/gates
log=var/gates/m5-client-demo.log
config= serial= keep=0
while (( $# )); do
  case $1 in
    --stack) (( $# >= 2 )) || { printf 'FAIL: missing private config path\n'; exit 2; }; config=$2; shift 2 ;;
    --serial) (( $# >= 2 )) || { printf 'FAIL: missing disposable emulator serial\n'; exit 2; }; serial=$2; shift 2 ;;
    --keep) keep=1; shift ;;
    *) printf 'FAIL: M5 client demo arguments\n'; exit 2 ;;
  esac
done
[[ -n $config && $serial =~ ^emulator-[0-9]+$ ]] || { printf 'FAIL: M5 client demo needs --stack private-config --serial disposable-emulator\n'; exit 2; }
cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  reap_stale_netsimd >>"$log" 2>&1 || true
  printf 'EXIT=%s\n' "$rc" >>"$log"
  if (( rc != 0 )); then printf 'FAIL: M5 client demo (diagnostics in var/gates)\n'; fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# Reap before any optional subsequent controller boot; never kills another active emulator.
reap_stale_netsimd >>"$log" 2>&1
if ! (cd client && ./gradlew --offline --console=plain --max-workers=4 :androidApp:assembleDebug :androidApp:assembleDebugAndroidTest :desktopApp:createDistributable) >"$log" 2>&1; then exit 1; fi
python3 -I deploy/scripts/m5-client-gate.py demo "$config" "$serial" "$keep" 2>>"$log"
