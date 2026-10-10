#!/usr/bin/env bash
# Controller-only laptop proof: scratch keys/releases, isolated stack, disposable emulator, private desktop HOME.
set -euo pipefail
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd -P)
cd "$root"
[[ $root != */var/wt/* ]] || { printf 'FAIL run update acceptance from the main tree\n'; exit 2; }
. scripts/env.sh
. scripts/emulator-common.sh
export PYTHONDONTWRITEBYTECODE=1
# Never interpolate dev or production configuration over the scratch stack's independently generated .env.
unset POSTGRES_PASSWORD PORCHLIGHT_MIGRATOR_PASSWORD PORCHLIGHT_APP_PASSWORD PORCHLIGHT_AGENT_PASSWORD PORCHLIGHT_BACKUP_PASSWORD PORCHLIGHT_VERSION
unset PORCHLIGHT_DATA_DIR PORCHLIGHT_RELEASE_REPO PORCHLIGHT_RELEASE_PUBLIC_KEY PORCHLIGHT_UPGRADE_COMMAND PORCHLIGHT_RELEASE_DIR
unset PORCHLIGHT_MIN_CLIENT_VERSION FUNNEL_ENABLED TS_HOSTNAME TAILSCALE_IMAGE_SHA256 TS_AUTHKEY_SPEC TS_AUTHKEY FUNNEL_IP
keep=0
case ${1:-} in '') ;; --keep) keep=1 ;; *) printf 'usage: update-acceptance.sh [--keep]\n'; exit 2 ;; esac
[[ $# -le 1 ]] || exit 2
fail() { printf 'FAIL %s\n' "$1"; exit 1; }
state=$root/var/update-gate
[[ -d $state && ! -L $state && -f $state/build.json ]] || fail 'run update-gate-build.sh first'
exec 9> "$state/.lock"
flock -n 9 || fail 'another update gate is running'
serial=emulator-5582
[[ -z $(adb -s "$serial" devices | python3 -I -c 'import sys;print("yes" if any(x.startswith("emulator-") for x in sys.stdin) else "")') ]] || fail 'shut down existing emulators first'
if pgrep -u "$(id -u)" -f '[q]emu-system|[e]mulator.*-avd' >/dev/null; then fail 'shut down starting emulators first'; fi
work=$(mktemp -d "$state/run-$(date -u +%Y%m%dT%H%M%SZ).XXXXXXXX"); chmod 700 "$work"
project=porchlight-update-gate-${work##*.}; project=${project,,}
export PORCHLIGHT_ROOT=$work/stack PORCHLIGHT_RUN_DIR=$work/run PORCHLIGHT_PROJECT=$project
export BACKEND_SUBNET=${UPDATE_GATE_BACKEND_SUBNET:-172.28.244.0/24} INGRESS_SUBNET=${UPDATE_GATE_INGRESS_SUBNET:-172.28.245.0/24}
emulator_pid='' mirror_pid='' apk_attempted=0 stack_attempted=0 stage=inventory
private() { local name=$1; shift; "$@" > "$work/$name.log" 2>&1; }
driver() { python3 -I "$root/deploy/scripts/update-gate.py" "$@"; }
latest() { driver latest "$work" "$1"; }
dc() {
  local compose=$PORCHLIGHT_ROOT/release-current/compose.yml
  [[ -f $compose ]] || compose=$state/dists/9.9.8/compose.yml
  docker compose -p "$project" --env-file "$PORCHLIGHT_ROOT/.env" -f "$compose" "$@"
}
cleanup() {
  local code=$? cleanup_failed=0
  trap - EXIT INT TERM
  if [[ -n $emulator_pid ]]; then
    # Private diagnostics: app class-only diagnostics plus platform installer/runtime errors.
    timeout 20 adb -s "$serial" logcat -d > "$work/logcat-all.txt" 2>&1 || true
    grep -i -E 'porchlight|packageinstaller|installersession|packagemanager|AndroidRuntime|TestRunner' "$work/logcat-all.txt" > "$work/logcat.log" 2>/dev/null || true
    rm -f -- "$work/logcat-all.txt"
    if ((apk_attempted)); then
      timeout 30 adb -s "$serial" uninstall app.porchlight.test > "$work/test-apk-cleanup.log" 2>&1 || cleanup_failed=1
      timeout 30 adb -s "$serial" uninstall app.porchlight > "$work/apk-cleanup.log" 2>&1 || cleanup_failed=1
    fi
    timeout 20 adb -s "$serial" emu kill > "$work/emulator-stop.log" 2>&1 || kill "$emulator_pid" 2>/dev/null || true
    for ((i=0; i<50; i++)); do kill -0 "$emulator_pid" 2>/dev/null || break; sleep 0.2; done
    kill -KILL "$emulator_pid" 2>/dev/null || true
    wait "$emulator_pid" 2>/dev/null || true
    reap_stale_netsimd
    if pgrep -u "$(id -u)" -x netsimd >/dev/null; then cleanup_failed=1; fi
  fi
  if [[ -n $mirror_pid ]]; then kill "$mirror_pid" 2>/dev/null || true; wait "$mirror_pid" 2>/dev/null || true; fi
  if ((stack_attempted)) && [[ -f $PORCHLIGHT_ROOT/.env ]]; then
    # Private diagnostics only (0700 run dir); never printed.
    dc logs --no-color --timestamps > "$work/compose.log" 2>&1 || true
    dc down --volumes > "$work/teardown.log" 2>&1 || cleanup_failed=1
    if (( ! keep && ! cleanup_failed )) && [[ -d $PORCHLIGHT_ROOT/pgdata ]]; then
      # Remove only this run's container-owned PostgreSQL bind files, never a named or production volume.
      docker run --rm --pull=never --network none --user 0 --entrypoint /bin/sh \
        -v "$work:$work" porchlight-backup:9.9.8 -c 'rm -rf -- "$1/stack/pgdata"' sh "$work" > "$work/cleanup.log" 2>&1 || cleanup_failed=1
    fi
  fi
  if (( ! keep && ! cleanup_failed )); then
    # Retain only private diagnostics, not credentials, dists, dumps or a desktop installation.
    if ! python3 -I - "$work" > "$work/remove.log" 2>&1 <<'PY'
import pathlib,shutil,sys
for p in pathlib.Path(sys.argv[1]).iterdir():
 if p.is_dir() and not p.is_symlink(): shutil.rmtree(p)
 elif p.suffix not in ('.log', '.xml'): p.unlink()
PY
    then cleanup_failed=1; fi
  fi
  if ((cleanup_failed)); then code=1; fi
  if ((code)); then printf 'FAIL update acceptance %s (private diagnostics under var/update-gate)\n' "$stage"
  else printf 'PASS update acceptance\n'; fi
  exit "$code"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
private inventory driver verify-build
old=$state/dists/9.9.8
new=$state/dists/9.9.9
read -r PG_PORT LAN_PORT ENROLL_PORT mirror_port < <(driver ports)
export PG_PORT LAN_PORT ENROLL_PORT LAN_BIND=127.0.0.1
export ENROLL_HOST=localhost:$ENROLL_PORT PORCHLIGHT_ENDPOINTS=https://localhost:$LAN_PORT
export ENROLL_CIDRS=$(python3 -I - "$BACKEND_SUBNET" "$INGRESS_SUBNET" <<'PY'
import ipaddress,sys
print('127.0.0.1/32,'+','.join(str(ipaddress.ip_network(net)[1])+'/32' for net in sys.argv[1:]))
PY
)
stage=mirror
private mirror driver mirror "$work"
python3 -I -m http.server "$mirror_port" --bind 127.0.0.1 --directory "$work/mirror" > "$work/mirror-server.log" 2>&1 &
mirror_pid=$!
# Test-only local mirror, never a remote host or the owner's configured release repository.
export PORCHLIGHT_RELEASE_BASE_URL=http://127.0.0.1:$mirror_port PORCHLIGHT_INSTALL_TESTING=1
private mirror-ready python3 -I - "$mirror_port" "$new/manifest.json" <<'PY'
import http.client,pathlib,sys,time
expected=pathlib.Path(sys.argv[2]).read_bytes()
for _ in range(30):
 c=http.client.HTTPConnection('127.0.0.1',int(sys.argv[1]),timeout=2)
 try:
  c.request('GET','/latest/download/manifest.json');r=c.getresponse()
  if r.status==200 and r.read()==expected: break
 except OSError: pass
 finally: c.close()
 time.sleep(1)
else: raise SystemExit('mirror not ready')
PY
kill -0 "$mirror_pid" || fail 'local mirror exited'
printf 'PASS local signed release mirror\n'
stage=install
private password python3 -I - "$work/admin.pass" <<'PY'
import os,secrets,sys
with open(os.open(sys.argv[1],os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600),'w') as f: f.write(secrets.token_urlsafe(32)+'\n')
PY
private age age-keygen -o "$work/backup-recovery.key"
recipient=$(age-keygen -y "$work/backup-recovery.key" 2> "$work/age-recipient.log")
stack_attempted=1
private install bash "$old/install.sh" server --source "$old" --root "$PORCHLIGHT_ROOT" --run-dir "$PORCHLIGHT_RUN_DIR" --project "$project" \
  --no-funnel --no-backup --admin-password-file "$work/admin.pass" --admin-username admin --admin-name 'Sam Rivera' \
  --household 'Update Gate Scratch' --tz UTC --backup-recipient "$recipient" --san-dns localhost --san-ip 127.0.0.1
# Set an explicit minimum so the 426 probe is meaningful; persist it across upgrade/rollback.
private settings python3 -I - "$PORCHLIGHT_ROOT/.env" <<'PY'
import pathlib,sys
p=pathlib.Path(sys.argv[1]);values={'PORCHLIGHT_RELEASE_REPO':'example-owner/porchlight','PORCHLIGHT_MIN_CLIENT_VERSION':'9.9.8'}
lines=[line for line in p.read_text().splitlines() if line.split('=',1)[0] not in values]
p.write_text('\n'.join(lines+[k+'='+v for k,v in values.items()])+'\n');p.chmod(0o600)
PY
private restart dc up -d --no-deps --force-recreate --wait --wait-timeout 120 api
private healthy dc exec -T api /usr/local/bin/porchlight healthcheck
private token python3 -I - "$PORCHLIGHT_ROOT/init.json" "$work/agent.token" <<'PY'
import json,pathlib,sys
pathlib.Path(sys.argv[2]).write_text(json.load(open(sys.argv[1]))['bootstrap_token'])
PY
porch=$old/porch-9.9.8-x86_64-linux-musl
cli() { "$porch" --socket "$PORCHLIGHT_RUN_DIR/agent.sock" --token-file "$work/agent.token" --json "$@"; }
export PORCHLIGHT_PASSWORD=$(< "$work/admin.pass")
private enroll cli dev-init --out "$work/devices" --user admin --devices desktop:desktop,probe:desktop --enroll-addr 127.0.0.1:$ENROLL_PORT
unset PORCHLIGHT_PASSWORD
printf 'PASS scratch server 9.9.8 and pinned TLS enrollment\n'
stage=probe
private probe-old driver probe "$work" "$LAN_PORT" 9.9.8
printf 'PASS client-release hashes and 426 recovery exemption\n'
cli items add --title 'Update acceptance item' --kind todo --visibility household > "$work/item.json" 2> "$work/item.log"
item=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["id"])' "$work/item.json")
assert_running() {
  local label=$1
  private "$label-health" dc exec -T api /usr/local/bin/porchlight healthcheck
  private "$label-version" dc exec -T api /usr/local/bin/porchlight --version
  cli items show "$item" > "$work/$label-item.json" 2> "$work/$label-item.log"
  private "$label-assert" python3 -I - "$work/$label-version.log" "$work/$label-item.json" "$PORCHLIGHT_ROOT/release-current/manifest.json" "$item" <<'PY'
import json,pathlib,sys
assert pathlib.Path(sys.argv[1]).read_text().split()==['porchlight','9.9.9']
row=json.load(open(sys.argv[2]));assert row['id']==sys.argv[4] and row['title']=='Update acceptance item'
assert json.load(open(sys.argv[3]))['version']=='9.9.9'
PY
  private "$label-release" driver probe "$work" "$LAN_PORT" 9.9.9
}
expect_log() { python3 -I - "$work/$1.log" "$2" <<'PY'
import pathlib,sys
assert sys.argv[2] in pathlib.Path(sys.argv[1]).read_text()
PY
}
stage=upgrade
private upgrade "$PORCHLIGHT_ROOT/porchlight-upgrade" 9.9.9
expect_log upgrade 'PASS upgrade 9.9.9'
porch=$new/porch-9.9.9-x86_64-linux-musl
assert_running upgraded
private clean-upgrade driver assert-clean-upgrade "$work"
printf 'PASS upgrade 9.9.9 and preserved item; no incoming payload or plain dump\n'
private same "$PORCHLIGHT_ROOT/porchlight-upgrade" 9.9.9
expect_log same 'PASS already at 9.9.9'
status=0; private downgrade "$PORCHLIGHT_ROOT/porchlight-upgrade" 9.9.8 || status=$?
[[ $status == 2 ]] || fail 'downgrade did not fail with the expected exit status'
expect_log downgrade 'refusing downgrade'
printf 'PASS already at 9.9.9 and refused downgrade\n'
stage=rollback
private latest latest 9.9.10
status=0; private rollback "$PORCHLIGHT_ROOT/porchlight-upgrade" latest || status=$?
[[ $status == 1 ]] || fail 'forced migration upgrade did not fail with exit 1'
expect_log rollback 'ROLLED BACK to 9.9.9'
private forced-migration python3 -I - "$PORCHLIGHT_ROOT/tmp" "$work/migration-failure.log" <<'PY'
import pathlib,sys
logs=[p for p in pathlib.Path(sys.argv[1]).glob('install.*/upgrade.log') if 'update gate: forced migration failure' in p.read_text()]
assert len(logs)==1, 'rollback was not caused by the forced migration'
pathlib.Path(sys.argv[2]).write_bytes(logs[0].read_bytes())
PY
assert_running rolled-back
private clean-rollback driver assert-clean-upgrade "$work" rollback
printf 'PASS automatic rollback to 9.9.9 with healthy API, item and client release; no plain dump\n'
stage=desktop
mkdir -m 700 "$work/home"
desktop() { env HOME="$work/home" XDG_DATA_HOME="$work/home/.local/share" XDG_CONFIG_HOME="$work/home/.config" XDG_STATE_HOME="$work/home/.local/state" "$@"; }
base=$work/home/.local/share/porchlight
private desktop-old desktop bash "$old/install.sh" desktop --source "$old"
private desktop-stage driver desktop-stage "$work" "$LAN_PORT"
private desktop-update desktop bash "$base/current/install.sh" desktop --source "$work/desktop-staging"
[[ $(readlink "$base/current") == releases/9.9.9 && $(readlink "$base/previous") == releases/9.9.8 ]] || fail 'desktop update links incorrect'
# Verify the byte refusal, not just a version refusal: tamper while the original same-version source is installed.
cp -a "$work/desktop-staging" "$work/desktop-tampered"
private tamper python3 -I - "$work/desktop-tampered/porchlight.svg" <<'PY'
import pathlib,sys
p=pathlib.Path(sys.argv[1]);data=bytearray(p.read_bytes());data[0]^=1;p.write_bytes(data)
PY
status=0; private desktop-tamper desktop bash "$base/current/install.sh" desktop --source "$work/desktop-tampered" || status=$?
[[ $status == 1 ]] || fail 'tampered desktop source did not fail verification'
expect_log desktop-tamper 'operations: validation or I/O failed'
[[ $(readlink "$base/current") == releases/9.9.9 && $(readlink "$base/previous") == releases/9.9.8 ]] || fail 'tamper changed desktop links'
private desktop-rollback desktop "$work/home/.local/bin/porchlight-update" --rollback
[[ $(readlink "$base/current") == releases/9.9.8 && $(readlink "$base/previous") == releases/9.9.9 ]] || fail 'desktop rollback links incorrect'
printf 'PASS desktop mTLS subset update, tamper refusal and rollback in scratch HOME\n'
stage=android
# Recheck just before starting, not only before the potentially lengthy server work.
[[ -z $(adb -s "$serial" devices | python3 -I -c 'import sys;print("yes" if any(x.startswith("emulator-") for x in sys.stdin) else "")') ]] || fail 'shut down existing emulators first'
if pgrep -u "$(id -u)" -f '[q]emu-system|[e]mulator.*-avd' >/dev/null; then fail 'shut down starting emulators first'; fi
private emulator-ports python3 -I - <<'PY'
import socket
sockets=[socket.socket(),socket.socket()]
for sock,port in zip(sockets,(5582,5583)): sock.bind(('127.0.0.1',port))
for sock in sockets: sock.close()
PY
reap_stale_netsimd
pgrep -u "$(id -u)" -x netsimd >/dev/null && fail 'stale netsimd did not exit'
emulator -avd porch-api35 -port 5582 -no-window -no-audio -no-snapshot -gpu swiftshader_indirect > "$work/emulator.log" 2>&1 &
emulator_pid=$!
ready=0
for ((i=0; i<120; i++)); do
  kill -0 "$emulator_pid" 2>/dev/null || fail 'porch-api35 emulator exited'
  if [[ $(adb -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r') == 1 ]]; then ready=1; break; fi
  sleep 2
done
((ready)) || fail 'porch-api35 boot timeout'
[[ $(adb -s "$serial" shell getprop ro.build.version.sdk | tr -d '\r') == 35 ]] || fail 'wrong emulator API'
private unlock adb -s "$serial" shell input keyevent 82
for package in app.porchlight.test app.porchlight; do
  if adb -s "$serial" shell pm path "$package" > "$work/apk-before.log" 2>&1 && [[ -s $work/apk-before.log ]]; then
    private apk-uninstall adb -s "$serial" uninstall "$package"
  fi
done
apk_attempted=1
private android-apk adb -s "$serial" install -r "$state/android/androidApp-debug.apk"
private android-test-apk adb -s "$serial" install -r "$state/android/androidApp-debug-androidTest.apk"
private reverse-lan adb -s "$serial" reverse tcp:$LAN_PORT tcp:$LAN_PORT
private reverse-enroll adb -s "$serial" reverse tcp:$ENROLL_PORT tcp:$ENROLL_PORT
instrument() {
  local name=$1; shift
  private "$name" timeout 240 adb -s "$serial" shell am instrument -w -e class "app.porchlight.android.$name" "$@" app.porchlight.test/androidx.test.runner.AndroidJUnitRunner
  python3 -I - "$work/$name.log" <<'PY'
import pathlib,re,sys
text=pathlib.Path(sys.argv[1]).read_text()
assert re.search(r'OK \([1-9][0-9]* tests?\)',text) and not any(s in text for s in ('FAILURES!!!','INSTRUMENTATION_FAILED','Process crashed'))
PY
}
instrument UpdateSignerInstrumentedTest
printf 'PASS Android real installed/archive signers and different-key refusal\n'
cli enroll --user admin --device update-gate-android > "$work/android-enroll.json" 2> "$work/android-enroll.log"
private android-input driver android-input "$work" "$serial"
private unknown-apps adb -s "$serial" shell appops set app.porchlight REQUEST_INSTALL_PACKAGES allow
instrument UpdateGateTest -e UPDATE_GATE_INPUT update-gate.json
printf 'PASS Android Available(9.9.9), verified download and committed session with system confirmation\n'
private android-confirm driver android-confirm "$work" "$serial"
printf 'PASS Android system-confirmed in-app update to 9.9.9\n'
stage=cleanup
