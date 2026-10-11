#!/usr/bin/env bash
# Run by the controller on the laptop with the dev stack and an unlocked Secret Service wallet.
umask 022
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
. "$root/scripts/env.sh"
export CARGO_NET_OFFLINE=true
private="$root/var/trust-e2e"
mkdir -p "$private"
chmod 700 "$private"
trap 'unset PORCHLIGHT_E2E_URI PORCHLIGHT_E2E_PASSWORD; rm -f "$private/enrollment.json"' EXIT
# Enrollment output contains a one-time token: it goes to private runtime storage, not stdout.
(umask 077; cd "$root/server"; cargo run --offline --locked -q -p porchlight-cli --bin porch -- \
  --socket "$root/var/run/agent.sock" --token-file "$root/var/bootstrap.pat" --json \
  enroll --user "${PORCHLIGHT_E2E_USER:-admin}" --device c1b-desktop-e2e > "$private/enrollment.json")
PORCHLIGHT_E2E_URI=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["uri"])' "$private/enrollment.json")
if [[ -z ${PORCHLIGHT_E2E_PASSWORD:-} ]]; then
  read -r -s -p 'Dev household password (not echoed): ' PORCHLIGHT_E2E_PASSWORD
  printf '\n'
fi
export PORCHLIGHT_E2E_URI PORCHLIGHT_E2E_PASSWORD
export PORCHLIGHT_E2E_REQUIRED=1
export PORCHLIGHT_KEYRING_TEST=1
cd "$root/client"
./gradlew --offline --max-workers=4 --console=plain --rerun-tasks :shared:jvmTest \
  --tests 'app.porchlight.shared.trust.DesktopE2eTest'
