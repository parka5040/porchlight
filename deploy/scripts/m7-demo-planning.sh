#!/usr/bin/env bash
# Controller-only M7 planning gate: isolated Compose stack, real HTTP and porch.
# Credentials and raw bodies/logs stay in private var/m7-demo-planning, never stdout.
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
for tool in docker curl openssl python3; do
  command -v "$tool" >/dev/null || { printf 'FAIL: setup missing tool %s\n' "$tool" >&2; exit 1; }
done
base="$root/var/m7-demo-planning"
envfile="$base/compose.env"
compose_ready=0
cleanup() {
  local rc=$?
  trap - ERR EXIT
  if (( compose_ready && ! keep )); then
    dc down -v --remove-orphans >>"$base/teardown.log" 2>&1 || rc=1
  fi
  if (( rc )); then printf 'FAIL: planning gate incomplete (private diagnostics under var/m7-demo-planning)\n' >&2; fi
  exit "$rc"
}
trap cleanup EXIT
trap 'printf "FAIL: planning setup/runtime at line %s (private diagnostics under var/m7-demo-planning)\n" "$LINENO" >&2' ERR
check() {
  if [[ $2 == "$3" ]]; then printf 'PASS: %s\n' "$1";
  else printf 'FAIL: %s\n' "$1" >&2; exit 1; fi
}
jget() {
  python3 - "$1" "$2" <<'PY'
import json,sys
v=json.load(open(sys.argv[1]))
for p in sys.argv[2].split('.'): v=v[int(p)] if isinstance(v,list) else v[p]
print(str(v).lower() if isinstance(v,bool) else v)
PY
}
unset DATABASE_URL PORCHLIGHT_SUPERUSER_URL PORCHLIGHT_MIGRATOR_URL PORCHLIGHT_DATABASE_URL PORCHLIGHT_AGENT_URL
unset PORCHLIGHT_TOKEN PORCHLIGHT_PASSWORD
export SQLX_OFFLINE=true CARGO_NET_OFFLINE=true
mkdir -p "$base"
chmod 700 "$base"
make_env() {
  {
    printf 'PORCHLIGHT_VERSION=%s\n' "$(tr -d '\r\n' < "$root/VERSION")"
    printf 'PORCHLIGHT_VAR=%s\n' "$base"
    printf 'PORCHLIGHT_UID=%s\nPORCHLIGHT_GID=%s\n' "$(id -u)" "$(id -g)"
    printf 'PG_PORT=17432\nLAN_PORT=20443\nENROLL_PORT=20444\nSIDECAR_PORT=21445\n'
    printf 'BACKEND_SUBNET=172.31.248.0/24\nBACKEND_GATEWAY=172.31.248.1\n'
    printf 'INGRESS_SUBNET=172.31.249.0/24\nINGRESS_GATEWAY=172.31.249.1\nSIDECAR_IP=172.31.249.10\n'
    printf 'ENROLL_CIDRS=172.31.248.1/32,172.31.249.1/32\n'
    printf 'ENROLL_HOST=localhost:20444\nPORCHLIGHT_ENDPOINTS=https://localhost:20443\n'
    for k in POSTGRES_PASSWORD PORCHLIGHT_MIGRATOR_PASSWORD PORCHLIGHT_APP_PASSWORD PORCHLIGHT_AGENT_PASSWORD PORCHLIGHT_BACKUP_PASSWORD PORCHLIGHT_INIT_ADMIN_PASSWORD; do
      printf '%s=%s\n' "$k" "$(openssl rand -hex 24)"
    done
  } >"$envfile"
  chmod 600 "$envfile"
  set -a
  . "$envfile"
  set +a
}
make_env
dc() { docker compose -p porchlight-m7planning -f "$root/deploy/compose.dev.yml" --env-file "$envfile" "$@"; }
compose_ready=1
dc down -v --remove-orphans >"$base/teardown.log" 2>&1
# Only the gate's own stopped bind state; never dev runtime state.
rm -rf -- "$base"
compose_ready=0
mkdir -m 700 -p "$base"
make_env
compose_ready=1
mkdir -p "$base/server" "$base/run" "$base/pgsock" "$base/empty-capath"
chmod 700 "$base/server" "$base/run" "$base/empty-capath"
chmod 1777 "$base/pgsock"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/server/target}
[[ $CARGO_TARGET_DIR = /* ]] || CARGO_TARGET_DIR="$root/$CARGO_TARGET_DIR"
CARGO_NET_OFFLINE=true cargo build -q --manifest-path "$root/server/Cargo.toml" --locked -p porchlight-cli --bin porch >"$base/build-cli.log" 2>&1
porch="$CARGO_TARGET_DIR/debug/porch"
dc up -d --wait postgres >"$base/stack.log" 2>&1
dc build api >>"$base/stack.log" 2>&1
dc up --no-deps --force-recreate --exit-code-from migrate migrate >>"$base/stack.log" 2>&1
"$root/scripts/bin/age-keygen" -o "$base/backup-recovery.key" >"$base/age.log" 2>&1
recipient=$("$root/scripts/bin/age-keygen" -y "$base/backup-recovery.key")
dc run --rm --no-deps -T -e PORCHLIGHT_INIT_ADMIN_PASSWORD api init --data-dir /data \
  --household 'M7 Planning Demo' --tz UTC --admin-username admin --admin-name 'Planning Admin' \
  --san-dns localhost --san-ip 127.0.0.1 --age-recipient "$recipient" --json >"$base/init.json" 2>"$base/init.log"
jget "$base/init.json" bootstrap_token >"$base/bootstrap.pat"
member=$(jget "$base/init.json" admin_member_id)
household=$(jget "$base/init.json" household_id)
dc up -d --wait api >>"$base/stack.log" 2>&1
export PORCHLIGHT_SOCKET="$base/run/agent.sock"
export PORCHLIGHT_PASSWORD="$PORCHLIGHT_INIT_ADMIN_PASSWORD"
cli() { "$porch" --json --token-file "$base/bootstrap.pat" "$@" 2>"$base/porch.log"; }
cli dev-init --out "$base/clients" --devices desktop:desktop >"$base/dev-init.json"
unset PORCHLIGHT_PASSWORD
client="$base/clients/desktop"
request() { # method path output [body] [If-Match]
  local method=$1 path=$2 output=$3 body=${4:-} version=${5:-}
  local args=()
  {
    printf 'Content-Type: application/json\nAuthorization: Bearer %s\n' "$(<"$client/session.token")"
    printf 'Porchlight-Client-Version: %s\n' "$(tr -d '\r\n' < "$root/VERSION")"
    printf 'Idempotency-Key: %s\n' "$(python3 -c 'import uuid; print(uuid.uuid4())')"
    [[ -z $version ]] || printf 'If-Match: %s\n' "$version"
  } >"$base/headers"
  [[ -z $body ]] || args+=(--data-binary "@$body")
  env -u SSLKEYLOGFILE -u SSL_CERT_DIR -u SSL_CERT_FILE -u CURL_CA_BUNDLE -u CURL_HOME \
    curl -q --silent --show-error --noproxy '*' --max-time 20 --tlsv1.3 --tls-max 1.3 \
    --cacert "$client/ca.crt" --capath "$base/empty-capath" --cert "$client/client.crt" --key "$client/client.key" \
    -H "@$base/headers" -X "$method" "${args[@]}" -o "$output" -w '%{http_code}' \
    "https://localhost:$LAN_PORT$path" 2>"$base/curl.log"
}
check 'HTTP availability defaults empty' "$(request GET "/v1/members/$member/availability" "$base/default.json")" 200
check 'availability default collection version' "$(jget "$base/default.json" version)" 0
python3 - "$base" <<'PY'
import datetime,json,pathlib,sys
p=pathlib.Path(sys.argv[1]); local=datetime.datetime.now(datetime.timezone.utc); today=local.date()
# Start two hours ahead; EDF is acceptably future regardless of time of day.
start=(local+datetime.timedelta(hours=2)).replace(minute=0,second=0,microsecond=0)
end=start+datetime.timedelta(hours=2)
(p/'windows.json').write_text(json.dumps({'windows':[{'weekday':i,'start':start.strftime('%H:%M:%S'),'end':end.strftime('%H:%M:%S')} for i in range(7)]}))
(p/'appointment-times.json').write_text(json.dumps({'start':start.strftime('%H:%M:%S'),'end':(start+datetime.timedelta(hours=1)).strftime('%H:%M:%S')}))
for n,est in enumerate([90,60]):
    (p/f'task{n}.json').write_text(json.dumps({'title':f'Planning task {n}','kind':'todo','time_mode':'floating','due_date':str(today+datetime.timedelta(days=10)),'est_minutes':est}))
PY
check 'HTTP availability replacement' "$(request PUT "/v1/members/$member/availability" "$base/set.json" "$base/windows.json" 0)" 200
cli schedule availability "$member" >"$base/cli-availability.json"
version=$(jget "$base/cli-availability.json" version)
cli schedule set-availability "$member" --file "$base/windows.json" --version "$version" >"$base/cli-set.json"
check 'CLI set-availability retains seven weekdays' "$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["windows"]))' "$base/cli-set.json")" 7
for n in 0 1; do check 'HTTP future task creation' "$(request POST /v1/items "$base/item$n.json" "$base/task$n.json")" 201; done
check 'load conflict returned by HTTP write' "$(python3 -c 'import json,sys; print(any(c["kind"]=="load" and c["other_item_id"] is None for c in json.load(open(sys.argv[1]))["conflicts"]))' "$base/item1.json")" True
cli schedule suggest >"$base/suggest.json"
check 'CLI EDF proposes both tasks' "$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["suggestions"]))' "$base/suggest.json")" 2
cli schedule list --limit 1 >"$base/list.json"
check 'CLI suggestions page has one row' "$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["items"]))' "$base/list.json")" 1
first=$(jget "$base/suggest.json" suggestions.0.id)
occ_version=$(jget "$base/suggest.json" suggestions.0.occurrence_version)
second=$(jget "$base/suggest.json" suggestions.1.id)
sug_version=$(jget "$base/suggest.json" suggestions.1.version)
cli schedule accept "$first" --version "$occ_version" >"$base/accept.json"
check 'CLI accepts occurrence span override' "$(jget "$base/accept.json" status)" accepted
cli schedule dismiss "$second" --version "$sug_version" >"$base/dismiss.json"
check 'CLI dismisses remaining proposal' "$(jget "$base/dismiss.json" status)" dismissed
# A dependent is a member without a login. This fixture is SQL only; the actual
# appointment/candidate read still goes through the real HTTP/CLI surfaces.
dc exec -T postgres psql -U postgres -d porchlight -At -v ON_ERROR_STOP=1 \
  -c "insert into porchlight.household_members(household_id,role,display_name,color,tzid) values('$household','dependent','Planning dependent','#D3869B','UTC') returning id" >"$base/dependent.txt" 2>"$base/sql.log"
dependent=$(python3 -c 'import pathlib,sys,uuid; print(uuid.UUID(pathlib.Path(sys.argv[1]).read_text().splitlines()[0]))' "$base/dependent.txt")
python3 - "$base/appointment.json" "$dependent" "$base/appointment-times.json" <<'PY'
import datetime,json,sys
day=datetime.datetime.now(datetime.timezone.utc).date()+datetime.timedelta(days=12)
times=json.load(open(sys.argv[3])); end_day=day+datetime.timedelta(days=times['end']<times['start'])
json.dump({'title':'Dependent appointment','kind':'appointment','time_mode':'floating','subject_member_id':sys.argv[2],'local_start':str(day)+'T'+times['start'],'local_end':str(end_day)+'T'+times['end']},open(sys.argv[1],'w'))
PY
check 'HTTP dependent appointment creation' "$(request POST /v1/items "$base/appointment-created.json" "$base/appointment.json")" 201
appointment=$(jget "$base/appointment-created.json" id)
cli schedule candidates "$appointment" >"$base/candidates.json"
check 'CLI dependent candidates are current' "$(jget "$base/candidates.json" pending_derivation)" false
check 'CLI finds a free dependent assignee' "$(python3 -c 'import json,sys; print(any(c["reason"]=="free_inside_availability" for c in json.load(open(sys.argv[1]))["candidates"]))' "$base/candidates.json")" True
printf 'PASS: m7 planning HTTP/CLI gate complete\n'
