#!/usr/bin/env bash
# Controller-only: KVM + cached AOSP API 35/36 images, ControllerOnly excluded.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
minified_only=0
if [[ $# == 1 && $1 == --minified ]]; then
  minified_only=1
elif [[ $# != 0 ]]; then
  printf 'usage: test-managed-clients.sh [--minified]\n' >&2
  exit 2
fi
. "$root/scripts/env.sh"
. "$root/scripts/emulator-common.sh"
reap_stale_netsimd
trap reap_stale_netsimd EXIT
cd "$root/client"
results="$root/client/androidApp/build/outputs/androidTest-results/managedDevice"
if (( ! minified_only )); then
  # Missing outputs force fresh results; a crashed runner must not reuse a previous green report.
  rm -rf "$results/debug"
  ./gradlew --offline --console=plain --max-workers=4 -Pkotlin.incremental=false :androidApp:verifyManagedDeviceFilter :androidApp:phonesGroupDebugAndroidTest
  python3 -I "$root/scripts/check-managed-results.py" "$results/debug" api35 api36
  reap_stale_netsimd
fi
# Release R8/resource settings plus the test runner's cross-APK ABI; no production signing key.
rm -rf "$results/minified"
./gradlew --offline --console=plain --max-workers=4 -Pkotlin.incremental=false -Pporchlight.minifiedSmoke=true \
  :androidApp:verifyManagedDeviceFilter :androidApp:phonesGroupMinifiedAndroidTest
python3 -I "$root/scripts/check-managed-results.py" "$results/minified" api35 api36
