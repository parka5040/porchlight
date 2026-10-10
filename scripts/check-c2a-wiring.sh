#!/usr/bin/env bash
# R25: memory-only fixture support must remain unreachable from production/release entry points.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
for source in "$root/client/androidApp/src/main" "$root/client/androidApp/src/release"; do
  [[ -d $source ]] || continue
  if grep -R -E 'app\.porchlight\.shared\.testing|ShellFixture' "$source"; then
    printf 'release sources must not reference the debug trust fixture\n' >&2; exit 1
  fi
done
# Imports and fully-qualified uses of shared test support are allowed only in the debug app.
references=$(grep -R -n 'app\.porchlight\.shared\.testing' "$root/client/shared/src" "$root/client/androidApp/src" "$root/client/desktopApp/src" || true)
while IFS= read -r reference; do
  [[ -n $reference ]] || continue
  case $reference in
    "$root/client/androidApp/src/debug/"*) ;;
    "$root/client/shared/src/androidMain/kotlin/app/porchlight/shared/testing/"*':package app.porchlight.shared.testing') ;;
    *) printf 'shared fixture reference outside debug app: %s\n' "$reference" >&2; exit 1 ;;
  esac
done <<< "$references"
grep -q 'check(LocalContext.current.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0)' \
  "$root/client/shared/src/androidMain/kotlin/app/porchlight/shared/testing/ShellFixture.kt"
python3 - "$root" <<'PY'
from pathlib import Path
import sys, xml.etree.ElementTree as ET
root = Path(sys.argv[1])
ns = '{http://schemas.android.com/apk/res/android}'
manifest = ET.parse(root / 'client/androidApp/src/debug/AndroidManifest.xml')
activities = [a for a in manifest.findall('./application/activity') if a.get(ns+'name') in ('.ShellFixtureActivity', 'app.porchlight.android.ShellFixtureActivity')]
assert len(activities) == 1 and activities[0].get(ns+'exported') == 'false', 'fixture must be debug-only and not exported'
for file in ['client/androidApp/src/main/kotlin/app/porchlight/android/MainActivity.kt',
             'client/shared/src/androidMain/kotlin/app/porchlight/shared/testing/ShellFixture.kt']:
    assert 'PorchlightAndroidRoot(' in (root / file).read_text(), 'production and fixture must use the same root'
print('PASS debug-only, non-exported, FLAG_DEBUGGABLE fixture and shared Android root')
PY
