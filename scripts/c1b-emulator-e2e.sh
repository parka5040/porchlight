#!/usr/bin/env bash
# Controller-only real enrollment through Android's 10.0.2.2 host alias; no URI/password in adb arguments.
umask 022
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
. "$root/scripts/env.sh"
export CARGO_NET_OFFLINE=true
serial=${1:-emulator-5554}
private="$root/var/trust-e2e"
mkdir -p "$private"; chmod 700 "$private"
api_overridden=0
cleanup() {
  rm -f "$private/emulator-enrollment.json" "$private/emulator-config.json"
  adb -s "$serial" shell run-as app.porchlight rm -f files/c1b-e2e.json >/dev/null 2>&1 || true
  unset PORCHLIGHT_E2E_PASSWORD
  if [[ $api_overridden == 1 ]]; then
    # Original inherited ENROLL_HOST (or dev.env if unset) is restored, including on failure.
    PORCHLIGHT_VAR="$root/var" docker compose -p porchlight-dev -f "$root/deploy/compose.dev.yml" \
      --env-file "$root/var/dev.env" up -d --force-recreate --no-deps --wait api
  fi
}
trap cleanup EXIT
# Compose command-line environment overrides the saved env file for this dev listener only.
api_overridden=1
ENROLL_HOST=10.0.2.2 PORCHLIGHT_VAR="$root/var" \
  docker compose -p porchlight-dev -f "$root/deploy/compose.dev.yml" --env-file "$root/var/dev.env" \
  up -d --force-recreate --no-deps --wait api
(umask 077; cd "$root/server"; cargo run --offline --locked -q -p porchlight-cli --bin porch -- \
  --socket "$root/var/run/agent.sock" --token-file "$root/var/bootstrap.pat" --json \
  enroll --user "${PORCHLIGHT_E2E_USER:-admin}" --device c1b-emulator-e2e > "$private/emulator-enrollment.json")
if [[ -z ${PORCHLIGHT_E2E_PASSWORD:-} ]]; then
  read -r -s -p 'Dev household password (not echoed): ' PORCHLIGHT_E2E_PASSWORD
  printf '\n'
fi
export PORCHLIGHT_E2E_PASSWORD PORCHLIGHT_E2E_REQUIRED=1
python3 - "$private/emulator-enrollment.json" "$private/emulator-config.json" <<'PY'
import json, os, sys
uri = json.load(open(sys.argv[1]))['uri']
fd = os.open(sys.argv[2], os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, 'w') as f:
    json.dump({'uri': uri, 'password': os.environ['PORCHLIGHT_E2E_PASSWORD']}, f)
os.chmod(sys.argv[2], 0o600)
PY
(cd "$root/client"; ./gradlew --offline --console=plain :androidApp:assembleDebug :androidApp:assembleDebugAndroidTest)
adb -s "$serial" install -r "$root/client/androidApp/build/outputs/apk/debug/androidApp-debug.apk"
adb -s "$serial" shell run-as app.porchlight mkdir -p files
adb -s "$serial" shell -T 'run-as app.porchlight sh -c "umask 077; cat > files/c1b-e2e.json"' < "$private/emulator-config.json"
(cd "$root/client"; ANDROID_SERIAL="$serial" ./gradlew --offline --console=plain \
  :androidApp:connectedDebugAndroidTest \
  -Pandroid.testInstrumentationRunnerArguments.PORCHLIGHT_E2E_REQUIRED=1 \
  -Pandroid.testInstrumentationRunnerArguments.notAnnotation=org.junit.Ignore \
  -Pandroid.testInstrumentationRunnerArguments.class=app.porchlight.android.trust.AndroidEnrollmentE2eTest)
