#!/usr/bin/env bash
# M2 Trust gate. Isolated Compose project/data/ports; stdout is safe for the public gate log.
# All credentials, raw HTTP bodies, stderr and stack diagnostics stay under var/m2-demo (0600).
set -Eeuo pipefail
umask 077
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"
keep=0
after_reboot=0
for arg in "$@"; do
  case $arg in
    --keep) keep=1 ;;
    --after-reboot) after_reboot=1 ;;
    *) printf 'usage: %s [--keep|--after-reboot]\n' "$0" >&2; exit 2 ;;
  esac
done
fails=0
check() { # check <number> <description> <safe actual> <expected>
  if [[ $3 == "$4" ]]; then
    printf 'PASS: %s %s\n' "$1" "$2"
  else
    # Never print arbitrary actual values: they may be a response containing credentials.
    printf 'FAIL: %s %s\n' "$1" "$2"
    fails=$((fails + 1))
  fi
}
jget() { # A jq-independent extractor; values go only to private files or shell variables.
  python3 - "$1" "$2" <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
for part in sys.argv[2].split('.'):
    v = v[int(part)] if isinstance(v, list) else v[part]
print(str(v).lower() if isinstance(v, bool) else v)
PY
}
healthy() {
  local service id state
  for service in postgres api sidecar; do
    id=$(dc ps -q "$service")
    [[ -n $id ]] || return 1
    state=$(docker inspect --format '{{.State.Status}} {{.State.Health.Status}}' "$id")
    [[ $state == 'running healthy' ]] || return 1
  done
}
finish() {
  if (( fails )); then printf 'm2-demo: %s check(s) FAILED\n' "$fails" >&2; return 1; fi
  printf 'm2-demo: all checks passed\n'
}

# Isolated curl: never load ambient curlrc, CA directories, or TLS key logging.
pinned_curl() { # <pinned CA> <remaining curl arguments>
  local ca=$1
  shift
  env -u SSLKEYLOGFILE -u SSL_CERT_DIR -u SSL_CERT_FILE -u CURL_CA_BUNDLE -u CURL_HOME \
    curl -q -H "Porchlight-Client-Version: $(tr -d '[:space:]' < "$root/VERSION")" --cacert "$ca" --capath "$curl_empty" "$@"
}
wrong_root_control() { # <LAN port> <private output directory>
  local port=$1 dir=$2 rc
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
    -subj /CN=WrongRoot -days 1 -keyout "$dir/wrong-root.key" -out "$dir/wrong-root.crt" \
    >"$dir/wrong-root.log" 2>&1
  if pinned_curl "$dir/wrong-root.crt" --silent --show-error --noproxy '*' --max-time 15 \
    --tlsv1.3 --tls-max 1.3 -o /dev/null "https://localhost:$port/v1/connection" \
    2>"$dir/wrong-root-curl.log"; then rc=0; else rc=$?; fi
  check tls 'wrong CA secret-free LAN request fails verification (curl 60)' "$rc" 60
}

if (( after_reboot )); then
  . "$root/scripts/dev-db.sh"
  # Human-attested reboot: no up/start command here can disguise a stopped stack.
  if healthy; then state=yes; else state=no; fi
  check reboot 'dev postgres, api and sidecar running and healthy (human-attested reboot)' "$state" yes
  client="$root/var/dev-clients/desktop"
  if [[ ! -r $client/session.token || ! -r $client/client.key || ! -r $client/client.crt || ! -r $client/ca.crt ]]; then
    check reboot 'desktop dev credentials present' no yes
    finish
    exit 1
  fi
  curl_empty=$(mktemp -d "$root/var/reboot-empty-capath.XXXXXX")
  headers="$root/var/reboot.headers"
  printf 'Authorization: Bearer %s\n' "$(<"$client/session.token")" > "$headers"
  if status=$(pinned_curl "$client/ca.crt" --silent --show-error --noproxy '*' --max-time 15 --tlsv1.3 --tls-max 1.3 \
    --cert "$client/client.crt" --key "$client/client.key" \
    -H "@$headers" -o "$root/var/reboot.connection.json" -w '%{http_code}' \
    "https://localhost:${LAN_PORT:-8443}/v1/connection" 2>"$root/var/reboot.curl.log"); then rc=0; else rc=$?; fi
  check reboot 'desktop session answers on dev LAN after reboot' "$status/$rc" 200/0
  if [[ $status == 200 && $rc == 0 ]]; then
    check reboot 'desktop transport is lan' "$(jget "$root/var/reboot.connection.json" transport)" lan
  fi
  wrong_root_control "${LAN_PORT:-8443}" "$root/var"
  rm -f "$headers"
  rmdir "$curl_empty"
  finish
  exit $?
fi

. "$root/scripts/dev-lan.sh"
dev_lan_resolve

for tool in docker curl openssl python3 timeout sha256sum; do
  command -v "$tool" >/dev/null || { printf 'FAIL: setup required tool missing: %s\n' "$tool" >&2; exit 1; }
done
base="$root/var/m2-demo"
stream_pid=''
probe_network=porchlight-m2demo-probe
probe_container=porchlight-m2demo-probe
cidrs_changed=0
compose_ready=0
remove_probe() {
  # Fixed, demo-only names also recover resources left by an interrupted previous run.
  docker rm -f "$probe_container" >>"$base/teardown.log" 2>&1 || true
  docker network rm "$probe_network" >>"$base/teardown.log" 2>&1 || true
}
cleanup() {
  local rc=$?
  trap - ERR EXIT
  if [[ -n $stream_pid ]] && kill -0 "$stream_pid" 2>/dev/null; then
    kill "$stream_pid" 2>/dev/null || true
    wait "$stream_pid" 2>/dev/null || true
  fi
  if [[ -d $base ]]; then remove_probe; fi
  if (( compose_ready )); then
    if (( keep && cidrs_changed )); then
      export ENROLL_CIDRS=$dev_cidrs
      dc up -d --no-deps --force-recreate --wait api >>"$base/stack.log" 2>&1 || rc=1
    fi
    if (( ! keep )); then dc down -v --remove-orphans >>"$base/teardown.log" 2>&1 || rc=1; fi
  fi
  if (( rc )); then
    printf 'FAIL: gate/setup/cleanup did not complete (private diagnostics under var/m2-demo)\n' >&2
  fi
  exit "$rc"
}
# Install before any destructive startup work; report only the line, never the failed command.
trap cleanup EXIT
trap 'printf "FAIL: setup/runtime operation at line %s (private diagnostics under var/m2-demo)\n" "$LINENO" >&2' ERR
unset DATABASE_URL PORCHLIGHT_SUPERUSER_URL PORCHLIGHT_MIGRATOR_URL PORCHLIGHT_DATABASE_URL PORCHLIGHT_AGENT_URL
export SQLX_OFFLINE=true
# Do not inherit the native dev DB URLs/Compose interpolation values from scripts/env.sh.
# Random passwords below are explicitly exported and override any caller environment.
mkdir -p "$base"
chmod 700 "$base"
envfile="$base/compose.env"
make_env() {
  {
    printf 'PORCHLIGHT_VERSION=%s\n' "$(tr -d '\r\n' < "$root/VERSION")"
    printf 'PORCHLIGHT_VAR=%s\n' "$base"
    printf 'PORCHLIGHT_UID=%s\nPORCHLIGHT_GID=%s\n' "$(id -u)" "$(id -g)"
    printf 'PG_PORT=15432\nLAN_PORT=18443\nENROLL_PORT=18444\nSIDECAR_PORT=19445\n'
    printf 'BACKEND_SUBNET=172.31.252.0/24\nBACKEND_GATEWAY=172.31.252.1\n'
    printf 'INGRESS_SUBNET=172.31.253.0/24\nINGRESS_GATEWAY=172.31.253.1\nSIDECAR_IP=172.31.253.10\n'
    printf 'ENROLL_CIDRS=%s,172.31.252.1/32,172.31.253.1/32\n' "$DEV_LAN_CIDR"
    printf 'ENROLL_HOST=localhost:18444\nPORCHLIGHT_ENDPOINTS=https://localhost:18443\n'
    for k in POSTGRES_PASSWORD PORCHLIGHT_MIGRATOR_PASSWORD PORCHLIGHT_APP_PASSWORD PORCHLIGHT_AGENT_PASSWORD PORCHLIGHT_BACKUP_PASSWORD PORCHLIGHT_INIT_ADMIN_PASSWORD; do
      printf '%s=%s\n' "$k" "$(openssl rand -hex 24)"
    done
  } > "$envfile"
  chmod 600 "$envfile"
  # Enrollment peer decisions log at DEBUG (O1-01: no per-connection INFO flood); section 11 inspects each probe's.
  printf 'services:\n  api:\n    environment:\n      RUST_LOG: info,porchlight_api::server=debug\n' > "$base/compose.m2.yml"
  set -a
  . "$envfile"
  set +a
}
make_env
dc() { docker compose -p porchlight-m2demo -f "$root/deploy/compose.dev.yml" -f "$base/compose.m2.yml" --env-file "$envfile" "$@"; }
compose_ready=1
# down needs interpolation even on the first invocation; then replace all bind state, never dev var/.
dc down -v --remove-orphans >"$base/teardown.log" 2>&1
remove_probe
rm -rf -- "$base"
compose_ready=0
mkdir -m 700 -p "$base"
make_env
compose_ready=1
mkdir -p "$base/server" "$base/run" "$base/pgsock" "$base/probes" "$base/backup-work"
chmod 700 "$base/server" "$base/run" "$base/probes" "$base/backup-work"
chmod 1777 "$base/pgsock"
curl_empty="$base/empty-capath"
mkdir -m 700 "$curl_empty"

# Build porch with the same locked workspace; no container shell is assumed for the distroless api.
CARGO_NET_OFFLINE=true cargo build -q --manifest-path "$root/server/Cargo.toml" --locked -p porchlight-cli --bin porch \
  >"$base/build-cli.log" 2>&1
porch="${CARGO_TARGET_DIR:-$root/server/target}/debug/porch"
dc up -d --wait postgres >"$base/stack.log" 2>&1
dc build api >>"$base/stack.log" 2>&1
dc up --no-deps --force-recreate --exit-code-from migrate migrate >>"$base/stack.log" 2>&1
"$root/scripts/bin/age-keygen" -o "$base/backup-recovery.key" >"$base/age.log" 2>&1
recipient=$("$root/scripts/bin/age-keygen" -y "$base/backup-recovery.key")
init_args=(init --data-dir /data --household 'M2 Demo Household' --admin-username admin --admin-name 'Demo Admin'
  --san-dns localhost --san-ip 127.0.0.1 --san-ip 10.0.2.2 --san-ip "$DEV_LAN_IP" --age-recipient "$recipient" --json)
# --no-deps uses the demo's networks and demo app-role URL; never the dev/native URL.
dc run --rm --no-deps -T -e PORCHLIGHT_INIT_ADMIN_PASSWORD api "${init_args[@]}" \
  >"$base/init.json" 2>"$base/init.log"
fp="sha256:$(openssl x509 -in "$base/server/pki/ca.crt" -outform DER | sha256sum | cut -d ' ' -f 1)"
check 1 'init fingerprint equals SHA-256 of CA DER' "$(jget "$base/init.json" ca_fingerprint)" "$fp"
jget "$base/init.json" bootstrap_token >"$base/bootstrap.pat"
chmod 600 "$base/bootstrap.pat"
# Snapshot EVERY data file and its mode, not just the certificates.
data_manifest() {
  python3 - "$base/server" <<'PY'
import hashlib, pathlib, stat, sys
root = pathlib.Path(sys.argv[1])
for p in sorted(root.rglob('*')):
    mode = stat.S_IMODE(p.stat().st_mode)
    if p.is_file():
        print(p.relative_to(root), oct(mode), hashlib.sha256(p.read_bytes()).hexdigest())
    elif p.is_dir():
        print(p.relative_to(root), oct(mode), 'directory')
PY
}
permissions=$(python3 - "$base/server" <<'PY'
import pathlib, stat, sys
r=pathlib.Path(sys.argv[1])
print('yes' if stat.S_IMODE(r.stat().st_mode)==0o700 and all(stat.S_IMODE(p.stat().st_mode)==(0o700 if p.is_dir() else 0o600) for p in r.rglob('*')) else 'no')
PY
)
check 1 'all init files 0600 and directories 0700' "$permissions" yes
data_manifest >"$base/before.manifest"
if dc run --rm --no-deps -T -e PORCHLIGHT_INIT_ADMIN_PASSWORD api "${init_args[@]}" >"$base/second-init.json" 2>"$base/second-init.log"; then rc=0; else rc=$?; fi
check 1 'second init exits 2' "$rc" 2
data_manifest >"$base/after.manifest"
if cmp -s "$base/before.manifest" "$base/after.manifest"; then identical=yes; else identical=no; fi
check 1 'second init leaves every data file identical' "$identical" yes
dc up -d --wait api sidecar >>"$base/stack.log" 2>&1
export PORCHLIGHT_SOCKET="$base/run/agent.sock"
export PORCHLIGHT_TOKEN="$(<"$base/bootstrap.pat")"
unset PORCHLIGHT_PASSWORD

# curl never receives a secret as a command-line argument: JSON bodies and headers are private files.
headers() { # <destination> <token file or empty> <idempotency key or empty>
  {
    printf 'Content-Type: application/json\n'
    if [[ -n $2 ]]; then printf 'Authorization: Bearer %s\n' "$(<"$2")"; fi
    if [[ -n $3 ]]; then printf 'Idempotency-Key: %s\n' "$3"; fi
  } > "$1"
}
request() { # <device A|B> <method> <path> <output> [body] [key] [tokenfile] [port]
  local device=$1 method=$2 path=$3 output=$4 body=${5:-} key=${6:-} tokenfile=${7:-} port=${8:-$LAN_PORT}
  local args=()
  headers "$base/request.headers" "$tokenfile" "$key"
  [[ -z $body ]] || args+=(--data-binary "@$body")
  pinned_curl "$base/$device/root.pem" --silent --show-error --noproxy '*' --max-time 15 --tlsv1.3 --tls-max 1.3 \
    --cert "$base/$device/client.crt" --key "$base/$device/client.key" \
    -H "@$base/request.headers" -X "$method" "${args[@]}" -o "$output" -w '%{http_code}' \
    "https://localhost:$port$path" 2>"$base/curl.log"
}

# 2. Pin the fetched root against the URI BEFORE sending enrollment token/password/CSR.
for device in A B; do
  mkdir -m 700 "$base/$device"
  "$porch" --json enroll --user admin --device "$device" >"$base/$device/uri.json" 2>"$base/porch.log"
  timeout 15 openssl s_client -connect "127.0.0.1:$ENROLL_PORT" -servername localhost -showcerts \
    </dev/null >"$base/$device/chain.txt" 2>"$base/$device/tls.log"
  python3 - "$base/$device" <<'PY'
import pathlib, re, sys
p=pathlib.Path(sys.argv[1])
certs=re.findall(r'-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----', (p/'chain.txt').read_text(), re.S)
if len(certs)<2: raise SystemExit('Enrollment chain must include root')
(p/'root.pem').write_text(certs[-1]+'\n')
(p/'leaf.pem').write_text(certs[0]+'\n')
PY
  uri_fp=$(python3 - "$base/$device/uri.json" <<'PY'
import json, sys, urllib.parse
print(urllib.parse.parse_qs(urllib.parse.urlparse(json.load(open(sys.argv[1]))['uri']).query)['ca'][0])
PY
)
  chain_fp="sha256:$(openssl x509 -in "$base/$device/root.pem" -outform DER | sha256sum | cut -d ' ' -f 1)"
  check 2 "$device enrollment root matches URI pin before any secrets" "$chain_fp" "$uri_fp"
  # Fail closed, rather than carrying on after reporting an incorrect pin.
  [[ $chain_fp == "$uri_fp" && $chain_fp == "$fp" ]] || exit 1
  openssl verify -CAfile "$base/$device/root.pem" -verify_hostname localhost "$base/$device/leaf.pem" >"$base/$device/verify.log" 2>&1
  openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj "/CN=$device" \
    -keyout "$base/$device/client.key" -out "$base/$device/client.csr" >"$base/$device/csr.log" 2>&1
  python3 - "$base/$device" "$device" <<'PY'
import json, pathlib, sys
# Password is supplied via the environment, not retained in logs.
import os
p=pathlib.Path(sys.argv[1])
v=json.load(open(p/'uri.json'))
(p/'complete.json').write_text(json.dumps({'token':v['token'], 'password':os.environ['PORCHLIGHT_INIT_ADMIN_PASSWORD'], 'csr':(p/'client.csr').read_text(), 'platform':'android' if sys.argv[2]=='A' else 'desktop'}))
PY
  headers "$base/enroll.headers" '' "enroll-$device"
  status=$(pinned_curl "$base/$device/root.pem" --silent --show-error --noproxy '*' --max-time 15 --tlsv1.3 --tls-max 1.3 \
    -H "@$base/enroll.headers" --data-binary "@$base/$device/complete.json" \
    -o "$base/$device/enrolled.json" -w '%{http_code}' "https://localhost:$ENROLL_PORT/v1/enrollment/complete" 2>"$base/curl.log")
  check 2 "$device enrollment returns 201" "$status" 201
  [[ $status == 201 ]] || exit 1
  jget "$base/$device/enrolled.json" certificate >"$base/$device/client.crt"
  jget "$base/$device/enrolled.json" session.token >"$base/$device/session.token"
done

status=$(request A GET /v1/me "$base/me.json" '' '' "$base/A/session.token")
check 3 'A /v1/me on LAN returns 200' "$status" 200
check 3 'connection.transport is lan' "$(jget "$base/me.json" connection.transport)" lan
status=$(request A GET /v1/connection "$base/funnel.json" '' '' "$base/A/session.token" "$SIDECAR_PORT")
check 4 'A /v1/connection via PROXY v2 sidecar returns 200' "$status" 200
check 4 'transport is funnel' "$(jget "$base/funnel.json" transport)" funnel
backend_gateway=$(docker network inspect porchlight-m2demo_backend --format '{{(index .IPAM.Config 0).Gateway}}')
ingress_gateway=$(docker network inspect porchlight-m2demo_ingress --format '{{(index .IPAM.Config 0).Gateway}}')
proxy_peer=$(python3 - "$base/funnel.json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))['client_addr'].rsplit(':',1)[0])
PY
)
check 4 'PROXY client address is observed ingress gateway, not sidecar IP' "$proxy_peer" "$ingress_gateway"
check 4 'PROXY source differs from sidecar' "$([[ $proxy_peer != "$SIDECAR_IP" ]] && printf yes || printf no)" yes
printf 'finding: PROXY source=%s; sidecar=%s\n' "$proxy_peer" "$SIDECAR_IP"

if status=$(pinned_curl "$base/A/root.pem" --silent --show-error --noproxy '*' --max-time 15 --tlsv1.3 --tls-max 1.3 \
  -o "$base/unenrolled.body" -w '%{http_code}' "https://localhost:$LAN_PORT/v1/me" 2>"$base/unenrolled.log"); then rc=0; else rc=$?; fi
check 5 'unenrolled connection has no HTTP status' "$status" 000
wrong_root_control "$LAN_PORT" "$base"
check 5 'unenrolled TLS fails (curl 35/56)' "$([[ $rc == 35 || $rc == 56 ]] && printf yes || printf no)" yes
status=$(request B GET /v1/me "$base/mismatch.json" '' '' "$base/A/session.token")
check 6 'A session with B certificate returns 401' "$status" 401
check 6 'session-device-mismatch problem' "$(jget "$base/mismatch.json" type)" urn:porchlight:problem:session-device-mismatch

python3 - "$base" <<'PY'
import json, os, pathlib, sys
p=pathlib.Path(sys.argv[1])
(p/'login.json').write_text(json.dumps({'password':os.environ['PORCHLIGHT_INIT_ADMIN_PASSWORD']}))
(p/'different-login.json').write_text(json.dumps({'password':os.environ['PORCHLIGHT_INIT_ADMIN_PASSWORD']+'-different'}))
PY
status=$(request A POST /v1/auth/login "$base/login-first.json" "$base/login.json" replay-login)
check 7 'first login returns 200' "$status" 200
status=$(request A POST /v1/auth/login "$base/login-replay.json" "$base/login.json" replay-login)
check 7 'identical login replay returns 200' "$status" 200
check 7 'identical login replay body is byte-for-byte equal' "$(cmp -s "$base/login-first.json" "$base/login-replay.json" && printf yes || printf no)" yes
jget "$base/login-first.json" session.token >"$base/A/session.token"
status=$(request A POST /v1/auth/login "$base/login-different.json" "$base/different-login.json" replay-login)
check 7 'same key different login body returns 422' "$status" 422
check 7 'idempotency-key-reused problem' "$(jget "$base/login-different.json" type)" urn:porchlight:problem:idempotency-key-reused

# Positive control: this exact B certificate/session must work before reset can invalidate it.
status=$(request B GET /v1/me "$base/B/before-reset.json" '' '' "$base/B/session.token")
check 8 'B session is valid before reset (200)' "$status" 200
"$porch" --json user reset-password admin >"$base/reset-code.json" 2>"$base/porch.log"
python3 - "$base" <<'PY'
import json, pathlib, secrets, sys
p=pathlib.Path(sys.argv[1]); password=secrets.token_hex(24)
(p/'new-password').write_text(password)
(p/'reset.json').write_text(json.dumps({'code':json.load(open(p/'reset-code.json'))['code'],'new_password':password}))
(p/'new-login.json').write_text(json.dumps({'password':password}))
PY
status=$(request A POST /v1/auth/reset-password "$base/reset-response.json" "$base/reset.json" reset-A)
check 8 'A redeems reset code with new password' "$status" 200
jget "$base/reset-response.json" session.token >"$base/A/session.token"
status=$(request B GET /v1/me "$base/B/old-session.json" '' '' "$base/B/session.token")
check 8 'reset invalidates B session (401)' "$status" 401
check 8 'reset invalidates B session: session-invalid problem' "$(jget "$base/B/old-session.json" type)" urn:porchlight:problem:session-invalid

status=$(request B POST /v1/auth/login "$base/B/login.json" "$base/new-login.json" login-B-after-reset)
check 9 'B logs in after reset' "$status" 200
jget "$base/B/login.json" session.token >"$base/B/session.token"
headers "$base/stream.headers" "$base/B/session.token" ''
pinned_curl "$base/B/root.pem" --silent --show-error --noproxy '*' --tlsv1.3 --tls-max 1.3 --no-buffer \
  --cert "$base/B/client.crt" --key "$base/B/client.key" \
  -H "@$base/stream.headers" -D "$base/stream.response.headers" "https://localhost:$LAN_PORT/v1/stream" \
  >"$base/stream.events" 2>"$base/stream.log" &
stream_pid=$!
for _ in $(seq 1 50); do
  if [[ -f $base/stream.events ]] && grep -Eq '^event: ?ready' "$base/stream.events"; then break; fi
  sleep 0.1
done
check 9 'stream is HTTP 200' "$(grep -Eq '^HTTP/1\.[01] 200' "$base/stream.response.headers" && printf yes || printf no)" yes
check 9 'stream content-type is text/event-stream' "$(grep -Eiq '^content-type: *text/event-stream' "$base/stream.response.headers" && printf yes || printf no)" yes
check 9 'stream emits ready event' "$(grep -Eq '^event: ?ready' "$base/stream.events" && printf yes || printf no)" yes
sleep 3
check 9 'negative control: stream still alive after 3 seconds' "$(kill -0 "$stream_pid" 2>/dev/null && printf yes || printf no)" yes
bid=$(jget "$base/B/enrolled.json" device_id)
"$porch" --json device revoke "$bid" >"$base/revoke-B.json" 2>"$base/porch.log"
for _ in $(seq 1 50); do kill -0 "$stream_pid" 2>/dev/null || break; sleep 0.1; done
check 9 'revocation closes stream within 5 seconds' "$(kill -0 "$stream_pid" 2>/dev/null && printf no || printf yes)" yes
if kill -0 "$stream_pid" 2>/dev/null; then kill "$stream_pid" 2>/dev/null || true; fi
wait "$stream_pid" 2>/dev/null || true
stream_pid=''
if status=$(request B GET /v1/me "$base/B/revoked.body" '' '' "$base/B/session.token"); then rc=0; else rc=$?; fi
check 9 'revoked B has no HTTP response on new handshake' "$status" 000
check 9 'revoked B TLS fails (curl 35/56)' "$([[ $rc == 35 || $rc == 56 ]] && printf yes || printf no)" yes

"$porch" --json token create --name demo-read --scope read --days 1 >"$base/read-pat.json" 2>"$base/porch.log"
read_token=$(jget "$base/read-pat.json" token)
read_id=$(jget "$base/read-pat.json" id)
if PORCHLIGHT_TOKEN="$read_token" "$porch" --json device list >"$base/read-list.json" 2>"$base/porch.log"; then rc=0; else rc=$?; fi
check 10 'read-only PAT device list exits 1' "$rc" 1
check 10 'read-only PAT is forbidden' "$(jget "$base/read-list.json" type)" urn:porchlight:problem:forbidden
if PORCHLIGHT_TOKEN="$read_token" "$porch" --json whoami >"$base/read-whoami-before.json" 2>"$base/porch.log"; then rc=0; else rc=$?; fi
check 10 'read-only PAT whoami succeeds before revoke (exit 0)' "$rc" 0
"$porch" --json token revoke "$read_id" >"$base/revoke-pat.json" 2>"$base/porch.log"
if PORCHLIGHT_TOKEN="$read_token" "$porch" --json whoami >"$base/read-whoami.json" 2>"$base/porch.log"; then rc=0; else rc=$?; fi
check 10 'revoked PAT whoami exits 1' "$rc" 1
check 10 'revoked PAT is unauthenticated' "$(jget "$base/read-whoami.json" type)" urn:porchlight:problem:unauthenticated

# 11. Capture only enrollment decisions; raw API logs remain private. Count avoids stale matches.
decisions() {
  dc logs --no-color --no-log-prefix api >"$base/probes/api.log" 2>&1
  python3 - "$base/probes/api.log" "$base/probes/decisions.json" <<'PY'
import json, re, sys
s=re.sub(r'\x1b\[[0-9;]*[A-Za-z]', '', open(sys.argv[1]).read())
rows=[]
for line in s.splitlines():
    if not re.search(r'transport="?enrollment"?',line): continue
    peer=re.search(r'\bpeer="?([0-9a-fA-F.:]+)',line)
    allowed=re.search(r'\ballowed=(true|false)',line)
    if peer and allowed: rows.append({'peer':peer[1], 'allowed':allowed[1]})
json.dump({'count':len(rows),'last':rows[-1] if rows else {'peer':'none','allowed':'none'}},open(sys.argv[2],'w'))
PY
}
probe_start() { decisions; before_count=$(jget "$base/probes/decisions.json" count); }
probe_decision() { # <public probe name>
  for _ in $(seq 1 30); do
    decisions
    count=$(jget "$base/probes/decisions.json" count)
    (( count > before_count )) && break
    sleep 0.1
  done
  peer=$(jget "$base/probes/decisions.json" last.peer)
  allowed=$(jget "$base/probes/decisions.json" last.allowed)
  check 11 "$1: new enrollment peer decision logged for this probe" "$((count > before_count))" 1
}
probe() { # <file label> <host> <public probe name>: verify cert presence, not HTTP auth.
  probe_start
  if timeout 15 openssl s_client -connect "$2:$ENROLL_PORT" -servername localhost -showcerts \
    </dev/null >"$base/probes/$1.chain" 2>"$base/probes/$1.log"; then probe_rc=0; else probe_rc=$?; fi
  if grep -q -- '-----BEGIN CERTIFICATE-----' "$base/probes/$1.chain"; then certificate=yes; else certificate=no; fi
  probe_decision "$3"
}
is_gateway() { [[ $1 == "$backend_gateway" || $1 == "$ingress_gateway" ]]; }
probe dev-loopback 127.0.0.1 'dev loopback'
check 11 'dev CIDRs: loopback enrollment TLS presents certificate' "$certificate" yes
check 11 'dev CIDRs: logged peer is an exact inspected demo gateway' "$(is_gateway "$peer" && printf yes || printf no)" yes
check 11 'dev CIDRs: gateway peer allowed' "$allowed" true
printf 'finding: dev loopback peer=%s; inspected backend=%s ingress=%s\n' "$peer" "$backend_gateway" "$ingress_gateway"
dev_cidrs=$ENROLL_CIDRS
cidrs_changed=1
export ENROLL_CIDRS=$DEV_LAN_CIDR
dc up -d --no-deps --force-recreate --wait api >>"$base/stack.log" 2>&1
probe production-loopback 127.0.0.1 'production loopback'
check 11 'production CIDRs: loopback closed before TLS (no certificate)' "$certificate" no
check 11 'production CIDRs: loopback refusal logged' "$allowed" false
check 11 'production CIDRs: refusal peer is an exact inspected gateway' "$(is_gateway "$peer" && printf yes || printf no)" yes
check 11 'production CIDRs: loopback closes, not a timeout' "$([[ $probe_rc != 124 ]] && printf yes || printf no)" yes
probe production-lan "$DEV_LAN_IP" 'LAN self-connect'
if [[ $peer == "$DEV_LAN_IP" ]]; then
  check 11 'LAN source preserved: peer allowed' "$allowed" true
  check 11 'LAN source preserved: TLS certificate presented' "$certificate" yes
  finding='Docker preserves LAN source'
elif is_gateway "$peer"; then
  check 11 'LAN source rewritten to gateway: peer refused' "$allowed" false
  check 11 'LAN source rewritten to gateway: no TLS certificate' "$certificate" no
  finding='Docker does not preserve LAN source on host self-connect'
else
  check 11 'LAN probe peer is laptop LAN address or inspected gateway' no yes
  finding='unexpected peer (requires investigation)'
fi
printf 'finding: production LAN self-connect peer=%s allowed=%s; %s\n' "$peer" "$allowed" "$finding"

# A valid, secret-free TLS ClientHello generated with Python's standard-library MemoryBIO.
python3 - "$base/probes/hello.bin" <<'PY'
import ssl, sys
ctx=ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
incoming=ssl.MemoryBIO(); outgoing=ssl.MemoryBIO()
s=ctx.wrap_bio(incoming,outgoing,server_side=False,server_hostname='localhost')
try: s.do_handshake()
except ssl.SSLWantReadError: pass
open(sys.argv[1],'wb').write(outgoing.read())
PY
docker network create "$probe_network" >"$base/probes/network.log" 2>&1
probe_start
if timeout 15 docker run --rm --name "$probe_container" --network "$probe_network" --entrypoint bash \
  -v "$base/probes:/probe:ro" postgres:18.6@sha256:5a5a84b19854a9ffaa54082c166ff4ec27473a361e496e5ea167f298f2da9722 \
  -c 'exec 3<>/dev/tcp/"$1"/"$2"; printf connected >&2; dd if=/probe/hello.bin >&3 2>/dev/null; dd bs=1 count=1 <&3 2>/dev/null' \
  probe "$DEV_LAN_IP" "$ENROLL_PORT" \
  >"$base/probes/outside.response" 2>"$base/probes/outside.log"; then outside_rc=0; else outside_rc=$?; fi
probe_decision 'separate network'
check 11 'separate-network container established TCP connection' "$(grep -q connected "$base/probes/outside.log" && printf yes || printf no)" yes
check 11 'separate-network peer refused before TLS' "$allowed" false
check 11 'separate-network TLS ClientHello gets no server bytes' "$([[ ! -s $base/probes/outside.response ]] && printf yes || printf no)" yes
check 11 'separate-network refusal closes, not a timeout' "$([[ $outside_rc != 124 ]] && printf yes || printf no)" yes
outside_lan=$(python3 -I - "$peer" "$DEV_LAN_CIDR" <<'PY'
import ipaddress, sys
try: print('yes' if ipaddress.ip_address(sys.argv[1]) in ipaddress.ip_network(sys.argv[2]) else 'no')
except ValueError: print('invalid')
PY
)
check 11 'separate-network logged peer is outside production LAN' "$outside_lan" no
printf 'finding: separate-network enrollment peer=%s allowed=%s\n' "$peer" "$allowed"
# Guard removal just as in EXIT cleanup; a timed-out probe may still have a container.
remove_probe
export ENROLL_CIDRS=$dev_cidrs
dc up -d --no-deps --force-recreate --wait api >>"$base/stack.log" 2>&1
cidrs_changed=0
probe restored-loopback 127.0.0.1 'restored'
check 11 'dev CIDRs restored: loopback TLS accepted' "$certificate/$allowed" yes/true

for service in postgres api sidecar; do
  id=$(dc ps -q "$service")
  policy=$(docker inspect --format '{{.HostConfig.RestartPolicy.Name}}' "$id")
  check 12 "$service restart policy is unless-stopped" "$policy" unless-stopped
done
# Docker must start at boot under whichever init system the laptop uses.
if command -v systemctl >/dev/null 2>&1 && systemctl is-enabled --quiet docker 2>/dev/null; then enabled=yes
elif rc-update show default >"$base/rc-update.log" 2>&1 && grep -Eq '^[[:space:]]*docker[[:space:]]*\|' "$base/rc-update.log"; then enabled=yes
else enabled=no; fi
check 12 'Docker starts at boot (systemd or OpenRC default runlevel)' "$enabled" yes
if ! finish; then exit 1; fi
