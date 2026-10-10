#!/usr/bin/env bash
# Shared by every emulator-booting entry point, including Gradle Managed Devices.
# Never stop a netsimd while this user's emulator is alive (including another AVD).
reap_stale_netsimd() {
  if pgrep -u "$(id -u)" -f '(^|/)(qemu-system[^ /]*|emulator)( |$)' >/dev/null 2>&1; then
    return 0
  fi
  pkill -u "$(id -u)" -x netsimd >/dev/null 2>&1 || true
  local attempt
  for attempt in 1 2 3 4 5; do
    pgrep -u "$(id -u)" -x netsimd >/dev/null 2>&1 || return 0
    sleep 0.2
  done
  # A stale daemon can ignore TERM. Recheck emulator ownership before escalation.
  if ! pgrep -u "$(id -u)" -f '(^|/)(qemu-system[^ /]*|emulator)( |$)' >/dev/null 2>&1; then
    pkill -KILL -u "$(id -u)" -x netsimd >/dev/null 2>&1 || true
  fi
}
