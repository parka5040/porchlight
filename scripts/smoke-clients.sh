#!/usr/bin/env bash
# Build both clients, launch the APK on an emulator and the packaged desktop app, and check both come up.
# Reuses a running emulator of that AVD, else boots it headless on port 5584 and shuts it down again afterwards.
#
# Usage: scripts/smoke-clients.sh [avd-name]   (default: porch-api35)
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
. "$root/scripts/env.sh"
. "$root/scripts/emulator-common.sh"
avd=${1:-porch-api35}
mkdir -p "$root/var"

cd "$root/client"
./gradlew --offline --console=plain --max-workers=4 -q :androidApp:assembleDebug :desktopApp:createReleaseDistributable

# Find a running emulator for this AVD; never touch physical devices or other AVDs.
serial=""
emu_pid=""
log="$root/var/emulator-$avd.log"
for s in $(adb devices | awk '$2=="device" && $1 ~ /^emulator-/ {print $1}'); do
  name=$(adb -s "$s" emu avd name 2>/dev/null | head -n1 | tr -d '\r' || true)
  if [[ $name == "$avd" ]]; then serial=$s; break; fi
done

desktop_pid=""
terminate() {  # TERM, wait up to 5 s, then KILL; safe to call repeatedly
  local pid=$1 i
  kill "$pid" 2>/dev/null || return 0
  for i in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.5
  done
  kill -9 "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}
profile=""
cleanup() {
  [[ -z $desktop_pid ]] || terminate "$desktop_pid"
  desktop_pid=""
  [[ -z $profile ]] || rm -rf -- "$profile"
  if [[ -n $emu_pid ]]; then
    adb -s "$serial" emu kill >/dev/null 2>&1 || true
    terminate "$emu_pid"
  fi
  reap_stale_netsimd
}
trap cleanup EXIT

if [[ -z $serial ]]; then
  reap_stale_netsimd
  serial=emulator-5584
  emulator -avd "$avd" -port 5584 -no-window -no-audio -no-boot-anim -no-snapshot-save >"$log" 2>&1 &
  emu_pid=$!
fi

deadline=$((SECONDS + 180))
until [[ $(adb -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true) == 1 ]]; do
  if [[ -n $emu_pid ]] && ! kill -0 "$emu_pid" 2>/dev/null; then
    echo "emulator $avd exited early, see $log" >&2; exit 1
  fi
  if (( SECONDS >= deadline )); then
    echo "timed out waiting for $avd ($serial) to boot, see $log" >&2; exit 1
  fi
  sleep 2
done
adb -s "$serial" install -r androidApp/build/outputs/apk/debug/androidApp-debug.apk >/dev/null
adb -s "$serial" shell am start -W -n app.porchlight/app.porchlight.android.MainActivity >/dev/null
sleep 2
activities=$(adb -s "$serial" shell dumpsys activity activities)
grep -q 'topResumedActivity=.*app.porchlight/.android.MainActivity' <<<"$activities" \
  || { echo "android: MainActivity is not resumed" >&2; exit 1; }
echo "android: MainActivity resumed on $avd"

launcher=desktopApp/build/compose/binaries/main-release/app/porchlight/bin/porchlight
# A private, unenrolled profile: never the user's real Porchlight data, trust, single-instance lock or home server.
# The session bus/display (XDG_RUNTIME_DIR, DISPLAY, WAYLAND_DISPLAY) stay real so tray/portal code paths run.
profile=$(mktemp -d "$root/var/smoke-desktop-profile.XXXXXX")
chmod 700 "$profile"
mkdir -p "$profile"/{data,config,state,cache}
env HOME="$profile" XDG_DATA_HOME="$profile/data" XDG_CONFIG_HOME="$profile/config" XDG_STATE_HOME="$profile/state" \
  XDG_CACHE_HOME="$profile/cache" PORCHLIGHT_KEY_STORE=file "$launcher" >"$root/var/smoke-desktop.log" 2>&1 &
desktop_pid=$!
sleep 8
kill -0 "$desktop_pid" 2>/dev/null || { echo "desktop: app exited early, see var/smoke-desktop.log" >&2; desktop_pid=""; exit 1; }
terminate "$desktop_pid"; desktop_pid=""
# A runtime image missing a JDK module only fails background coroutines, so "stayed up" alone is not enough.
if grep -nE 'NoClassDefFoundError|ClassNotFoundException' "$root/var/smoke-desktop.log" >&2; then
  echo "desktop: packaged runtime is missing classes (add the JDK module to desktopApp nativeDistributions.modules)" >&2; exit 1
fi
echo "desktop: packaged app stayed up"
