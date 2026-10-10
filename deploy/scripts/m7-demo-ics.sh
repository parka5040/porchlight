#!/usr/bin/env bash
# Isolated M7 ICS gate. All credentials/diagnostics stay under var/m7-ics-demo.
# Production SSRF refuses loopback; injected TLS tests cover successful feeds.
set -Eeuo pipefail
umask 077
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
keep=0
for arg in "$@"; do
  case $arg in
    --keep) keep=1 ;;
    *) printf 'usage: %s [--keep]\n' "$0" >&2; exit 2 ;;
  esac
done
base="$root/var/m7-ics-demo"
envfile="$base/compose.env"
compose_ready=0
fails=0
# Do not source scripts/env.sh here. Native DB credentials and caller Compose
# interpolation must never reach porch, curl, cargo or our diagnostic helpers.
unset DATABASE_URL PORCHLIGHT_SUPERUSER_URL PORCHLIGHT_MIGRATOR_URL PORCHLIGHT_DATABASE_URL PORCHLIGHT_AGENT_URL
unset POSTGRES_PASSWORD PORCHLIGHT_MIGRATOR_PASSWORD PORCHLIGHT_APP_PASSWORD PORCHLIGHT_AGENT_PASSWORD PORCHLIGHT_BACKUP_PASSWORD PORCHLIGHT_INIT_ADMIN_PASSWORD
unset PGPASSWORD PGPASSFILE PGSERVICE PGSERVICEFILE PGHOST PGPORT PGUSER PGDATABASE
unset PORCHLIGHT_TOKEN PORCHLIGHT_PASSWORD PORCHLIGHT_SOCKET PORCHLIGHT_TOKEN_FILE
unset PORCHLIGHT_VAR PORCHLIGHT_UID PORCHLIGHT_GID PG_PORT LAN_PORT ENROLL_PORT SIDECAR_PORT
unset BACKEND_SUBNET BACKEND_GATEWAY INGRESS_SUBNET INGRESS_GATEWAY SIDECAR_IP ENROLL_CIDRS ENROLL_HOST PORCHLIGHT_ENDPOINTS
export SQLX_OFFLINE=true
jget() {
  python3 - "$1" "$2" <<'PYJSON'
import json,sys
v=json.load(open(sys.argv[1]))
for k in sys.argv[2].split('.'): v=v[int(k)] if isinstance(v,list) else v[k]
print(str(v).lower() if isinstance(v,bool) else v)
PYJSON
}
check() {
  if [[ $2 == "$3" ]]; then printf 'PASS: %s\n' "$1"
  else printf 'FAIL: %s\n' "$1" >&2; fails=$((fails+1)); fi
}
dc() { docker compose -p porchlight-m7icsdemo -f "$root/deploy/compose.dev.yml" --env-file "$envfile" "$@"; }
cleanup() {
  local rc=$?
  trap - ERR EXIT
  if (( compose_ready && ! keep )); then dc down -v --remove-orphans >>"$base/teardown.log" 2>&1 || rc=1; fi
  if (( rc )); then printf 'FAIL: M7 gate/setup/cleanup (private diagnostics under var/m7-ics-demo)\n' >&2; fi
  exit "$rc"
}
trap cleanup EXIT
trap 'printf "FAIL: M7 runtime operation at line %s (private diagnostics under var/m7-ics-demo)\n" "$LINENO" >&2' ERR
for tool in docker openssl python3 cargo; do
  command -v "$tool" >/dev/null || { printf 'FAIL: setup required tool missing\n' >&2; exit 1; }
done
# Recover only this gate's previous project, never the shared dev database.
if [[ -f $envfile ]]; then dc down -v --remove-orphans >"$base/teardown.log" 2>&1; fi
rm -rf -- "$base"
mkdir -m 700 -p "$base"
{
  printf 'PORCHLIGHT_VERSION=%s\n' "$(tr -d '\r\n' < "$root/VERSION")"
  printf 'PORCHLIGHT_VAR=%s\n' "$base"
  printf 'PORCHLIGHT_UID=%s\nPORCHLIGHT_GID=%s\n' "$(id -u)" "$(id -g)"
  printf 'PG_PORT=25432\nLAN_PORT=28443\nENROLL_PORT=28444\nSIDECAR_PORT=29445\n'
  printf 'BACKEND_SUBNET=172.31.248.0/24\nBACKEND_GATEWAY=172.31.248.1\n'
  printf 'INGRESS_SUBNET=172.31.249.0/24\nINGRESS_GATEWAY=172.31.249.1\nSIDECAR_IP=172.31.249.10\n'
  printf 'ENROLL_CIDRS=172.31.248.1/32,172.31.249.1/32\nENROLL_HOST=localhost:28444\nPORCHLIGHT_ENDPOINTS=https://localhost:28443\n'
  for k in POSTGRES_PASSWORD PORCHLIGHT_MIGRATOR_PASSWORD PORCHLIGHT_APP_PASSWORD PORCHLIGHT_AGENT_PASSWORD PORCHLIGHT_BACKUP_PASSWORD PORCHLIGHT_INIT_ADMIN_PASSWORD; do
    printf '%s=%s\n' "$k" "$(openssl rand -hex 24)"
  done
} > "$envfile"
chmod 600 "$envfile"
# Compose reads the private file; no host process inherits these passwords.
compose_ready=1
mkdir -m 700 -p "$base/server" "$base/run" "$base/backup-work" "$base/pgsock"
chmod 1777 "$base/pgsock"
CARGO_NET_OFFLINE=true cargo build -q --manifest-path server/Cargo.toml --locked -p porchlight-cli --bin porch >"$base/build.log" 2>&1
porch="${CARGO_TARGET_DIR:-$root/server/target}/debug/porch"
dc up -d --wait postgres >"$base/stack.log" 2>&1
dc build api >>"$base/stack.log" 2>&1
dc up --no-deps --force-recreate --exit-code-from migrate migrate >>"$base/stack.log" 2>&1
"$root/scripts/bin/age-keygen" -o "$base/backup-recovery.key" >"$base/age.log" 2>&1
recipient=$("$root/scripts/bin/age-keygen" -y "$base/backup-recovery.key")
# The init-only overlay delivers the initial password inside the container,
# never via an exported host variable or command-line credential.
printf 'services:\n  api:\n    environment:\n      PORCHLIGHT_INIT_ADMIN_PASSWORD: ${PORCHLIGHT_INIT_ADMIN_PASSWORD:?missing init credential}\n' > "$base/init.yml"
docker compose -p porchlight-m7icsdemo -f "$root/deploy/compose.dev.yml" -f "$base/init.yml" --env-file "$envfile" \
  run --rm --no-deps -T api init --data-dir /data --household 'M7 Demo Household' --admin-username admin --admin-name 'Demo Admin' \
  --san-dns localhost --san-ip 127.0.0.1 --age-recipient "$recipient" --json >"$base/init.json" 2>"$base/init.log"
jget "$base/init.json" bootstrap_token > "$base/bootstrap.pat"
chmod 600 "$base/bootstrap.pat"
dc up -d --wait api sidecar >>"$base/stack.log" 2>&1
connection=(--socket "$base/run/agent.sock" --token-file "$base/bootstrap.pat")
"$porch" "${connection[@]}" --json whoami >"$base/whoami.json" 2>"$base/porch.log"
member=$(jget "$base/whoami.json" member_id)
run=$(python3 -c 'import uuid; print(uuid.uuid4())')
python3 - "$base/input.ics" "$run" <<'PY'
import datetime,sys
start=(datetime.datetime.now(datetime.timezone.utc)+datetime.timedelta(days=2)).replace(hour=10,minute=0,second=0,microsecond=0)
end=start+datetime.timedelta(hours=1)
open(sys.argv[1],'w').write('BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:'+sys.argv[2]+'\r\nDTSTART:'+start.strftime('%Y%m%dT%H%M%SZ')+'\r\nDTEND:'+end.strftime('%Y%m%dT%H%M%SZ')+'\r\nSUMMARY:M7 gate\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n')
PY
"$porch" "${connection[@]}" --json import ics "$base/input.ics" --member "$member" --scope "$run" --preview >"$base/preview.json" 2>>"$base/porch.log"
check 'porch import preview creates one planned item' "$(jget "$base/preview.json" created)" 1
"$porch" "${connection[@]}" --json import ics "$base/input.ics" --member "$member" --scope "$run" >"$base/import.json" 2>>"$base/porch.log"
item_id=$(jget "$base/import.json" item_ids.0)
check 'porch import commits one item' "$(jget "$base/import.json" created)" 1
"$porch" "${connection[@]}" --json import ics "$base/input.ics" --member "$member" --scope "$run" >"$base/reimport.json" 2>>"$base/porch.log"
check 'identical porch reimport reports unchanged' "$(jget "$base/reimport.json" unchanged)" 1
# Literal loopback requires no external DNS/network and must be refused in production.
"$porch" "${connection[@]}" --json feed add --name "m7-$run" --url 'http://127.0.0.1:1/calendar' --allow-http --member "$member" >"$base/add.json" 2>>"$base/porch.log"
feed_id=$(jget "$base/add.json" id)
check 'member feed defaults private' "$(jget "$base/add.json" visibility)" private
"$porch" "${connection[@]}" --json feed list >"$base/list.json" 2>>"$base/porch.log"
version=$(python3 - "$base/list.json" "$feed_id" <<'PY'
import json,sys
print(next(r['version'] for r in json.load(open(sys.argv[1]))['items'] if r['id']==sys.argv[2]))
PY
)
# Optimistic preconditions can race the source's initial scheduler fetch. Retry
# with the listed current version; only controlled labels go to stdout.
for (( retry=0; retry<3; retry++ )); do
  if "$porch" "${connection[@]}" --json feed refresh "$feed_id" --if-match "$version" >"$base/refresh.json" 2>>"$base/porch.log"; then break; fi
  [[ $(jget "$base/refresh.json" type) == urn:porchlight:problem:version-conflict ]] || exit 1
  version=$(jget "$base/refresh.json" current.version)
done
check 'porch refresh queues work' "$(jget "$base/refresh.json" last_status)" queued
status=''
for (( attempt=0; attempt<90; attempt++ )); do
  "$porch" "${connection[@]}" --json feed list >"$base/poll.json" 2>>"$base/porch.log"
  python3 - "$base/poll.json" "$feed_id" "$base/source.json" <<'PY'
import json,sys
json.dump(next(r for r in json.load(open(sys.argv[1]))['items'] if r['id']==sys.argv[2]),open(sys.argv[3],'w'))
PY
  status=$(jget "$base/source.json" last_status)
  [[ $status == failed ]] && break
  sleep 1
done
check 'running scheduler records refresh failure' "$status" failed
check 'production SSRF refuses loopback' "$(jget "$base/source.json" last_error)" ssrf_refused
version=$(jget "$base/source.json" version)
"$porch" "${connection[@]}" --json feed rm "$feed_id" --if-match "$version" >"$base/remove.json" 2>>"$base/porch.log"
check 'porch feed removal disables subscription' "$(jget "$base/remove.json" enabled)" false
if (( fails )); then printf 'FAIL: M7 ICS/feed gate (%s checks)\n' "$fails" >&2; exit 1; fi
printf 'PASS: M7 ICS/feed isolated-stack gate\n'
