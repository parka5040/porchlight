#!/usr/bin/env bash
# Controller only. Always creates a NEW AOSP AVD; --keep preserves artifacts/AVD, never running processes.
set -euo pipefail
umask 022
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
. scripts/env.sh
. scripts/emulator-common.sh
config= api=35 keep=0
while (( $# )); do
  case $1 in
    --stack) (( $# >= 2 )) || { printf 'FAIL: missing private config path\n'; exit 2; }; config=$2; shift 2 ;;
    --api) (( $# >= 2 )) || { printf 'FAIL: missing disposable API level\n'; exit 2; }; api=$2; shift 2 ;;
    --keep) keep=1; shift ;;
    *) printf 'FAIL: M5 soak arguments\n'; exit 2 ;;
  esac
done
[[ -n $config && $api =~ ^(35|36)$ ]] || { printf 'FAIL: M5 soak needs --stack private-config and API35 or API36\n'; exit 2; }
mkdir -p var/gates
run=$(mktemp -d "$root/var/gates/m5-soak-XXXXXXXX")
chmod 700 "$run"
log="$run/controller.log"
avd="porch-m5-${run##*-}"
export ANDROID_AVD_HOME="$run/avds"
mkdir -p "$ANDROID_AVD_HOME"
serial=emulator-5586
emu_pid= created=0
terminate() {
  local pid=$1 attempt
  kill "$pid" 2>/dev/null || true
  for attempt in {1..20}; do kill -0 "$pid" 2>/dev/null || break; sleep 0.25; done
  kill -KILL "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}
cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  if [[ -n $emu_pid ]]; then
    adb -s "$serial" emu kill >>"$log" 2>&1 || true
    terminate "$emu_pid"
  fi
  reap_stale_netsimd >>"$log" 2>&1 || true
  if (( created && ! keep )); then avdmanager delete avd -n "$avd" >>"$log" 2>&1 || true; fi
  printf 'EXIT=%s\n' "$rc" >>"$log"
  if (( rc != 0 )); then printf 'FAIL: M5 fresh-AVD soak (diagnostics in var/gates)\n'; fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# Never attach to a pre-existing emulator or delete an owner's AVD.
if adb devices 2>>"$log" | grep -q "^$serial[[:space:]]"; then exit 1; fi
reap_stale_netsimd >>"$log" 2>&1
if ! printf 'no\n' | avdmanager create avd -n "$avd" -p "$run/avd" -k "system-images;android-$api;default;x86_64" -d pixel_6 >"$log" 2>&1; then exit 1; fi
created=1
# Visible emulator window is required for the controller's real PIN/UI evidence checkpoints.
emulator -avd "$avd" -port 5586 -no-audio -no-boot-anim -no-snapshot -wipe-data >>"$log" 2>&1 &
emu_pid=$!
deadline=$((SECONDS + 240))
until [[ $(adb -s "$serial" shell getprop sys.boot_completed 2>>"$log" | tr -d '\r') == 1 ]]; do
  kill -0 "$emu_pid" 2>/dev/null || exit 1
  (( SECONDS < deadline )) || exit 1
  sleep 2
done
printf 'PASS: M5 booted fresh disposable API%s AVD\n' "$api"
if ! (cd client && ./gradlew --offline --console=plain --max-workers=4 :androidApp:assembleDebug :androidApp:assembleDebugAndroidTest :desktopApp:createDistributable) >>"$log" 2>&1; then exit 1; fi
python3 -I deploy/scripts/m5-client-gate.py soak "$config" "$serial" "$keep" 2>>"$log"
