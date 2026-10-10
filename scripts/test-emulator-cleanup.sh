#!/usr/bin/env bash
# Deterministic process seam: no actual emulator/netsimd is touched and no real sleeps.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
. "$root/scripts/emulator-common.sh"
active=0
stubborn=0
calls=0
waits=0
bad_calls=0
emulator_pattern=''
pgrep() {
  if [[ $* == *" -f "* ]]; then emulator_pattern=${!#}; [[ $active == 1 ]]; else [[ $stubborn == 1 ]]; fi
}
pkill() {
  # The reaper deliberately calls pkill under || true, which disables bash errexit in this function.
  # Record invalid arguments and assert OUTSIDE that conditional context.
  if [[ $* != "-u $(id -u) -x netsimd" && $* != "-KILL -u $(id -u) -x netsimd" ]]; then
    bad_calls=$((bad_calls + 1)); return 1
  fi
  calls=$((calls + 1))
  if [[ $1 == -KILL ]]; then stubborn=0; fi
}
sleep() { if [[ $1 != 0.2 ]]; then bad_calls=$((bad_calls + 1)); fi; waits=$((waits + 1)); }
pkill incorrect arguments || true
[[ $bad_calls == 1 && $calls == 0 ]] # Prove the assertion seam detects a swallowed mock failure.
bad_calls=0
reap_stale_netsimd
[[ $calls == 1 && $waits == 0 && $bad_calls == 0 ]]
for cmd in '/sdk/emulator/emulator -avd porch-api35' '/sdk/emulator/qemu/linux-x86_64/qemu-system-x86_64 -avd gradle' 'emulator -no-window'; do
  [[ $cmd =~ $emulator_pattern ]]
done
for cmd in '/sdk/emulator/netsimd --port 123' 'bash scripts/test-emulator-cleanup.sh' 'bash scripts/smoke-clients.sh'; do
  [[ ! $cmd =~ $emulator_pattern ]]
done
active=1
reap_stale_netsimd
[[ $calls == 1 && $bad_calls == 0 ]]
active=0
reap_stale_netsimd
[[ $calls == 2 && $bad_calls == 0 ]]
stubborn=1
reap_stale_netsimd
[[ $calls == 4 && $stubborn == 0 && $waits == 5 && $bad_calls == 0 ]]
printf 'emulator cleanup: invalid args detected, emulator regex checked, stale/shutdown reaped, active preserved, stubborn daemon killed\n'
