#!/usr/bin/env bash
# Laptop Trust stack: postgres -> migrate -> explicit init -> api + sidecar.
# Secrets and command output containing credentials stay under var/, never on stdout.
# Refuse before shell options, root resolution, or filesystem side effects.
if [ -z "${BASH_VERSION:-}" ]; then
  printf '%s: bash is required; refusing without side effects\n' "dev-stack" >&2
  return 2 2>/dev/null || exit 2
fi
if [[ ${BASH_SOURCE[0]} != "$0" && $- == *i* ]]; then
  printf '%s: interactive sourcing is not supported\n' "dev-stack" >&2
  return 2
fi
script_dir=$(dirname -- "${BASH_SOURCE[0]}") || { return 2 2>/dev/null || exit 2; }
root=$(cd -- "$script_dir/.." && pwd -P) || { return 2 2>/dev/null || exit 2; }
if [[ $root == / || ! -f $root/scripts/dev-db.sh ]]; then
  printf '%s: invalid project root; refusing\n' "dev-stack" >&2
  return 2 2>/dev/null || exit 2
fi
set -euo pipefail
umask 077
. "$root/scripts/dev-db.sh"
. "$root/scripts/env.sh"
export CARGO_NET_OFFLINE=${CARGO_NET_OFFLINE:-true}
cd "$root"
. "$root/scripts/dev-lan.sh"
# Resolve before init/up side effects; explicit overrides support VPN and multi-interface hosts.
case ${1:-} in init|up) dev_lan_config ;; esac

migrate_stack() {
  dc up -d --wait postgres
  dc build api
  # Re-run the one-shot even if a previous container has already exited successfully.
  dc up --no-deps --force-recreate --exit-code-from migrate migrate
}
init_household() {
  if [[ -e $root/var/server/pki ]]; then
    printf 'dev-stack: refusing init: var/server/pki already exists\n' >&2
    return 2
  fi
  if [[ ! -e $root/var/backup-recovery.key ]]; then
    "$root/scripts/bin/age-keygen" -o "$root/var/backup-recovery.key" >"$root/var/age-keygen.log" 2>&1
  fi
  chmod 600 "$root/var/backup-recovery.key"
  local recipient
  recipient=$("$root/scripts/bin/age-keygen" -y "$root/var/backup-recovery.key")
  # stdout includes the bootstrap PAT: capture before extracting, including on failed init.
  if ! PORCHLIGHT_INIT_ADMIN_PASSWORD="$PORCHLIGHT_DEV_ADMIN_PASSWORD" \
    cargo run -q -p porchlight-api --bin porchlight --manifest-path "$root/server/Cargo.toml" --locked -- \
    init --data-dir "$root/var/server" --household 'Dev Household' --admin-username admin \
    --admin-name 'Dev Admin' --san-dns localhost --san-ip 127.0.0.1 --san-ip 10.0.2.2 \
    --san-ip "$DEV_LAN_IP" --age-recipient "$recipient" --json \
    >"$root/var/init.json" 2>"$root/var/init.log"; then
    printf 'dev-stack: init failed (private diagnostics: var/init.log); M1 data may require reset\n' >&2
    return 1
  fi
  python3 - "$root/var/init.json" "$root/var/bootstrap.pat" <<'PY'
import json, os, sys
value = json.load(open(sys.argv[1]))
fd = os.open(sys.argv[2], os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, 'w') as f:
    f.write(value['bootstrap_token'] + '\n')
os.chmod(sys.argv[2], 0o600)
print('dev-stack: initialized; bootstrap PAT saved privately to var/bootstrap.pat')
PY
  rm -f "$root/var/init.json"
}

case ${1:-} in
  init)
    [[ ! -e $root/var/server/pki ]] || { printf 'dev-stack: already initialized\n' >&2; exit 2; }
    migrate_stack
    init_household
    ;;
  up)
    migrate_stack
    if [[ ! -e $root/var/server/pki ]]; then init_household; fi
    dc up -d --wait api sidecar
    printf 'dev-stack: postgres, api and sidecar healthy\n'
    ;;
  dev-init)
    [[ -S $root/var/run/agent.sock && -f $root/var/bootstrap.pat ]] || {
      printf 'dev-stack: run up first (agent socket/bootstrap PAT missing)\n' >&2; exit 1;
    }
    cargo build -q -p porchlight-cli --bin porch --manifest-path "$root/server/Cargo.toml" --locked
    if ! PORCHLIGHT_SOCKET="$root/var/run/agent.sock" PORCHLIGHT_TOKEN="$(<"$root/var/bootstrap.pat")" \
      PORCHLIGHT_PASSWORD="$PORCHLIGHT_DEV_ADMIN_PASSWORD" "$root/server/target/debug/porch" \
      --json dev-init --out "$root/var/dev-clients" --enroll-addr "127.0.0.1:${ENROLL_PORT:-8444}" \
      >"$root/var/dev-init.json" 2>"$root/var/dev-init.log"; then
      printf 'dev-stack: dev-init failed (private diagnostics: var/dev-init.log); never overwrites clients\n' >&2
      exit 1
    fi
    printf 'dev-stack: emulator and desktop credentials saved under var/dev-clients\n'
    ;;
  down) dc stop ;;
  reset)
    # DEV ONLY: never follows PORCHLIGHT_VAR to a different project or directory.
    dc stop api sidecar
    "$root/scripts/dev-db.sh" up
    "$root/scripts/dev-db.sh" reset
    dc build api
    dc up --no-deps --force-recreate --exit-code-from migrate migrate
    rm -rf -- "$root/var/server" "$root/var/run" "$root/var/dev-clients"
    rm -f -- "$root/var/bootstrap.pat" "$root/var/init.json" "$root/var/dev-init.json"
    mkdir -m 700 -p "$root/var/server" "$root/var/run"
    printf 'dev-stack: dev test data reset and migrated; run up, then dev-init\n'
    ;;
  *) printf 'usage: %s init|up|dev-init|down|reset\n' "$0" >&2; exit 2 ;;
esac
