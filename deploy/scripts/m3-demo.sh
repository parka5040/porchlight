#!/usr/bin/env bash
# M3 gate: isolated Compose state, private diagnostics, no secrets on stdout.
set -Eeuo pipefail
umask 077
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
keep=0
for arg in "$@"; do case "$arg" in --keep) keep=1 ;; *) printf 'usage: m3-demo.sh [--keep]\n' >&2; exit 2 ;; esac; done
for tool in docker openssl python3; do command -v "$tool" >/dev/null || { printf 'FAIL: M3 setup tool unavailable\n' >&2; exit 1; }; done
base="$root/var/m3-demo"
mkdir -p "$base"; chmod 700 "$base"
envfile="$base/compose.env"
make_env() {
  {
    printf 'PORCHLIGHT_VERSION=%s\n' "$(tr -d '\r\n' < "$root/VERSION")"
    printf 'PORCHLIGHT_VAR=%s\n' "$base"
    printf 'PORCHLIGHT_UID=%s\nPORCHLIGHT_GID=%s\n' "$(id -u)" "$(id -g)"
    printf 'PG_PORT=25432\nLAN_PORT=28443\nENROLL_PORT=28444\nSIDECAR_PORT=29445\n'
    printf 'BACKEND_SUBNET=172.31.254.0/24\nBACKEND_GATEWAY=172.31.254.1\n'
    printf 'INGRESS_SUBNET=172.31.255.0/24\nINGRESS_GATEWAY=172.31.255.1\nSIDECAR_IP=172.31.255.10\n'
    printf 'ENROLL_CIDRS=172.31.254.1/32,172.31.255.1/32\n'
    printf 'ENROLL_HOST=localhost:28444\nPORCHLIGHT_ENDPOINTS=https://localhost:28443\n'
    for k in POSTGRES_PASSWORD PORCHLIGHT_MIGRATOR_PASSWORD PORCHLIGHT_APP_PASSWORD PORCHLIGHT_AGENT_PASSWORD PORCHLIGHT_BACKUP_PASSWORD PORCHLIGHT_INIT_ADMIN_PASSWORD; do
      printf '%s=%s\n' "$k" "$(openssl rand -hex 24)"
    done
  } > "$envfile"
  chmod 600 "$envfile"
  set -a; . "$envfile"; set +a
}
unset DATABASE_URL PORCHLIGHT_SUPERUSER_URL PORCHLIGHT_MIGRATOR_URL PORCHLIGHT_DATABASE_URL PORCHLIGHT_AGENT_URL
export SQLX_OFFLINE=true
make_env
dc() { docker compose -p porchlight-m3demo -f "$root/deploy/compose.dev.yml" --env-file "$envfile" "$@"; }
ready=1
cleanup() {
  local rc=$?
  trap - ERR EXIT
  if (( ready && ! keep )); then dc down -v --remove-orphans >>"$base/teardown.log" 2>&1 || rc=1; fi
  if (( rc )); then printf 'FAIL: M3 gate (private diagnostics under var/m3-demo)\n' >&2; fi
  exit "$rc"
}
trap cleanup EXIT
trap 'printf "FAIL: M3 operation at line %s (private diagnostics under var/m3-demo)\n" "$LINENO" >&2' ERR
dc down -v --remove-orphans >"$base/teardown.log" 2>&1
rm -rf -- "$base"
mkdir -m 700 -p "$base"
make_env
mkdir -p "$base/server" "$base/run" "$base/pgsock" "$base/backup-work"
chmod 700 "$base/server" "$base/run" "$base/backup-work"
chmod 1777 "$base/pgsock"
CARGO_NET_OFFLINE=true cargo build -q --manifest-path "$root/server/Cargo.toml" --locked -p porchlight-cli --bin porch >"$base/build-cli.log" 2>&1
porch="${CARGO_TARGET_DIR:-$root/server/target}/debug/porch"
dc up -d --wait postgres >"$base/stack.log" 2>&1
dc build api >>"$base/stack.log" 2>&1
dc up --no-deps --force-recreate --exit-code-from migrate migrate >>"$base/stack.log" 2>&1
"$root/scripts/bin/age-keygen" -o "$base/backup-recovery.key" >"$base/age.log" 2>&1
recipient=$("$root/scripts/bin/age-keygen" -y "$base/backup-recovery.key")
dc run --rm --no-deps -T -e PORCHLIGHT_INIT_ADMIN_PASSWORD api init --data-dir /data --household 'M3 Demo' --admin-username admin --admin-name 'Demo Admin' --tz UTC --san-dns localhost --san-ip 127.0.0.1 --age-recipient "$recipient" --json >"$base/init.json" 2>"$base/init.log"
python3 - "$base/init.json" "$base/bootstrap.pat" <<'PY'
import json,sys,pathlib
pathlib.Path(sys.argv[2]).write_text(json.load(open(sys.argv[1]))['bootstrap_token']+'\n')
PY
chmod 600 "$base/bootstrap.pat"
dc up -d --wait api sidecar >>"$base/stack.log" 2>&1
python3 "$root/deploy/scripts/m3-demo.py" "$root" "$base" "$porch"
if [[ -d "$root/client/contract-check" ]]; then
  # The toolchain and cache are set before Gradle; this doesn't alter Compose state.
  . "$root/scripts/env.sh"
  (cd "$root/client" && ./gradlew --offline --console=plain :contract-check:build) >"$base/contract-check.log" 2>&1
  printf 'PASS: M3 contract-check builds\n'
else
  printf 'SKIP: M3 contract-check module absent (controller reruns after C2 merge)\n'
fi
printf 'm3-demo: all checks passed\n'
