#!/usr/bin/env bash
# Controller-only: KVM + cached AOSP API 35/36 images, ControllerOnly excluded.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
. "$root/scripts/env.sh"
. "$root/scripts/emulator-common.sh"
reap_stale_netsimd
trap reap_stale_netsimd EXIT
cd "$root/client"
./gradlew --offline --console=plain --max-workers=4 :androidApp:verifyManagedDeviceFilter :androidApp:phonesGroupDebugAndroidTest
