#!/usr/bin/env bash
# Laptop/controller M4 accessibility + managed devices + packaged launch evidence.
# No deployed host is contacted; logs stay in var until the controller reviews/copies them.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
. "$root/scripts/env.sh"
mkdir -p "$root/var/gates"
log="$root/var/gates/m4-ci.log"
# A foreground pipeline awaits tee, so EXIT evidence is flushed before a controller copies the log.
(
trap 'rc=$?; printf "M4 EXIT=%s\n" "$rc"' EXIT
printf 'M4: host accessibility (both themes, 200%%), API35/API36, packaged desktop + APK\n'
(cd "$root/client" && ./gradlew --offline --console=plain --max-workers=4 :shared:jvmTest :shared:testAndroidHostTest \
  :androidApp:assembleDebug :androidApp:assembleDebugAndroidTest :desktopApp:jar :androidApp:lintDebug :shared:lintAndroidMain)
"$root/scripts/test-managed-clients.sh"
"$root/scripts/smoke-clients.sh" porch-api35
printf 'M4 gate: OK\n'
) 2>&1 | tee "$log"
