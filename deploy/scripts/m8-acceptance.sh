#!/usr/bin/env bash
# Controller-only laptop gate. Signed dist artifacts, no dev stack or physical devices.
set -euo pipefail
umask 022
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
. scripts/env.sh
. scripts/emulator-common.sh
# Copy/dev role values must never leak into the independently generated scratch stack.
unset POSTGRES_PASSWORD PORCHLIGHT_MIGRATOR_PASSWORD PORCHLIGHT_APP_PASSWORD PORCHLIGHT_AGENT_PASSWORD PORCHLIGHT_BACKUP_PASSWORD PORCHLIGHT_VERSION
keep=0 source='' newer='' identity=''
while (($#)); do
  case $1 in
    --source) source=${2:?}; shift 2 ;;
    --upgrade-source) newer=${2:?}; shift 2 ;;
    --identity) identity=${2:?}; shift 2 ;;
    --keep) keep=1; shift ;;
    *) printf 'FAIL usage: --source dist/OLD --upgrade-source dist/NEW --identity var/backup-recovery.key [--keep]\n'; exit 2 ;;
  esac
done
fail() { printf 'FAIL %s\n' "$1"; exit 1; }
[[ -n $source && -n $newer && -s $identity ]] || fail 'release sources and recovery identity required'
source=$(realpath "$source"); newer=$(realpath "$newer"); identity=$(realpath "$identity")
[[ $source == "$root"/dist/* && $newer == "$root"/dist/* && $identity == "$root"/var/* ]] || fail 'sources must be clean dist releases and identity under var'
[[ -x $source/install.sh && -x $newer/install.sh ]] || fail 'generated release installers missing'
. deploy/lib/operations.sh
old=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$source/manifest.json")
new=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$newer/manifest.json")
(( $(version_code "$new") > $(version_code "$old") )) || fail 'upgrade requires a genuinely newer build'
[[ ! -e $root/var/restore-drill ]] || fail 'inspect existing restore-drill before acceptance'
# Do not kill another user's active emulator/netsim. Reap only when no emulator exists.
[[ -z $(adb devices | python3 -I -c 'import sys;print("yes" if any(x.startswith("emulator-") for x in sys.stdin) else "")') ]] || fail 'shut down existing emulators first'
if pgrep -u "$(id -u)" -f '[q]emu-system|[e]mulator.*-avd' >/dev/null; then fail 'shut down starting emulators first'; fi
work=$(mktemp -d "$root/var/m8-acceptance.XXXXXXXX"); chmod 700 "$work"
project=porchlight-m8-${work##*.}
project=${project,,}
export PORCHLIGHT_ROOT=$work/stack PORCHLIGHT_RUN_DIR=$work/run PORCHLIGHT_PROJECT=$project
export PG_PORT=${M8_PG_PORT:-56432} LAN_PORT=${M8_LAN_PORT:-19443} ENROLL_PORT=${M8_ENROLL_PORT:-19444}
export LAN_BIND=127.0.0.1 ENROLL_HOST=localhost:$ENROLL_PORT PORCHLIGHT_ENDPOINTS=https://localhost:$LAN_PORT
export BACKEND_SUBNET=${M8_BACKEND_SUBNET:-172.28.242.0/24} INGRESS_SUBNET=${M8_INGRESS_SUBNET:-172.28.243.0/24}
export ENROLL_CIDRS=$(python3 -I - "$BACKEND_SUBNET" "$INGRESS_SUBNET" <<'PY'
import ipaddress,sys
print('127.0.0.1/32,'+','.join(str(ipaddress.ip_network(net)[1])+'/32' for net in sys.argv[1:]))
PY
)
# env.sh sets a dev DATA_DIR; never mount it in the scratch stack.
unset PORCHLIGHT_DATA_DIR
emulator_pid='' serial=emulator-5580 release_install_attempted=0
stage=preflight
private() { local name=$1; shift; "$@" > "$work/$name.log" 2>&1; }
dc() { docker compose -p "$project" --env-file "$PORCHLIGHT_ROOT/.env" -f "$PORCHLIGHT_ROOT/release-current/compose.yml" "$@"; }
cleanup() {
  local code=$?
  trap - EXIT INT TERM
  if [[ -n $emulator_pid ]]; then
    # Clean the release signer off the shared development AVD before shutdown, even after a later failure/signal.
    # Marked before install so a partly successful install is covered too. --keep explicitly preserves it.
    if (( ! keep && release_install_attempted )); then
      timeout 30 adb -s "$serial" uninstall app.porchlight > "$work/apk-cleanup.log" 2>&1 || code=1
    fi
    timeout 20 adb -s "$serial" emu kill > "$work/emulator-stop.log" 2>&1 || kill "$emulator_pid" 2>/dev/null || true
    for ((i=0; i<50; i++)); do kill -0 "$emulator_pid" 2>/dev/null || break; sleep 0.2; done
    kill -KILL "$emulator_pid" 2>/dev/null || true
    wait "$emulator_pid" 2>/dev/null || true
    # After shutdown, no emulator may still own netsimd.
    reap_stale_netsimd
    if pgrep -u "$(id -u)" -x netsimd >/dev/null; then
      printf 'FAIL stale netsimd survived emulator shutdown\n'
      code=1
    fi
  fi
  if (( ! keep )); then
    if [[ -f $PORCHLIGHT_ROOT/.env && -L $PORCHLIGHT_ROOT/release-current ]]; then
      dc down --volumes > "$work/teardown.log" 2>&1 || code=1
      # postgres bind files are container-owned; use the verified release's backup image to remove only this scratch pgdata.
      local active_version
      active_version=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$PORCHLIGHT_ROOT/release-current/manifest.json") || code=1
      docker run --rm --pull=never --user 0 --entrypoint /bin/sh -v "$work:$work" "porchlight-backup:$active_version" -c 'rm -rf -- "$1/stack/pgdata"' sh "$work" > "$work/cleanup.log" 2>&1 || code=1
    fi
    if ((code == 0)); then rm -rf -- "$work" || code=1; fi
  fi
  if ((code)); then printf 'FAIL M8 %s (private diagnostics retained under var)\n' "$stage"; else printf 'PASS M8 acceptance\n'; fi
  exit "$code"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
stage=install
# Scratch password generated privately; never a command argument or console output.
python3 -I - "$work/admin.pass" <<'PY'
import os,secrets,sys
with open(os.open(sys.argv[1],os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600),'w') as f: f.write(secrets.token_urlsafe(32)+'\n')
PY
recipient=$(age-keygen -y "$identity" 2> "$work/recipient.log")
args=(--root "$PORCHLIGHT_ROOT" --run-dir "$PORCHLIGHT_RUN_DIR" --project "$project" --no-funnel)
private install bash "$source/install.sh" server --source "$source" "${args[@]}" --no-backup \
  --admin-password-file "$work/admin.pass" --admin-username admin --admin-name 'M8 Admin' --household 'M8 Scratch' --tz UTC \
  --backup-recipient "$recipient" --san-dns localhost --san-ip 127.0.0.1
printf 'PASS clean signed server install\n'
porch=$source/porch-$old-x86_64-linux-musl
python3 -I - "$PORCHLIGHT_ROOT/init.json" "$work/agent.token" <<'PY'
import json,os,sys
with open(os.open(sys.argv[2],os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600),'w') as f: f.write(json.load(open(sys.argv[1]))['bootstrap_token'])
PY
cli() { "$porch" --socket "$PORCHLIGHT_RUN_DIR/agent.sock" --token-file "$work/agent.token" --json "$@"; }
export PORCHLIGHT_PASSWORD=$(< "$work/admin.pass")
stage=enrollment
private enroll cli dev-init --out "$work/devices" --user admin --devices desktop:desktop --enroll-addr 127.0.0.1:$ENROLL_PORT
unset PORCHLIGHT_PASSWORD
printf 'PASS porch pinned TLS enrollment\n'
stage=functional
cli items add --title 'M8 acceptance item' --kind todo --visibility household > "$work/item.json" 2> "$work/item.log"
item=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["id"])' "$work/item.json")
cli items list > "$work/listed-items.json" 2> "$work/listed-items.log"
python3 -I - "$work/listed-items.json" "$item" <<'PY'
import json,sys
assert any(row['id']==sys.argv[2] and row['title']=='M8 acceptance item' for row in json.load(open(sys.argv[1]))['items'])
PY
cli lists add 'M8 checklist' --item-id "$item" > "$work/list.json" 2> "$work/list.log"
list=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["id"])' "$work/list.json")
cli lists add-entry "$list" 'M8 entry' > "$work/entry.json" 2> "$work/entry.log"
python3 -I - "$work/entry.json" "$list" <<'PY'
import json,sys
entry=json.load(open(sys.argv[1]));assert entry['list_id']==sys.argv[2] and entry['label']=='M8 entry'
PY
printf 'M8 attachment bytes\n' > "$work/attachment.txt"
cli attach "$item" "$work/attachment.txt" > "$work/attachment.json" 2> "$work/attachment.log"
attachment=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["id"])' "$work/attachment.json")
private comments cli comments add "$item" 'M8 comment'
private member-export cli export --member admin --out "$work/member.json"
private export cli export --household --out "$work/household.tar.gz"
private complete python3 -I deploy/lib/verify-export.py "$work/household.tar.gz"
printf 'PASS porch items list attachment comment member and complete household export\n'
stage=backup
private backup bash "$source/install.sh" backup "${args[@]}"
cli backups > "$work/server-backups.json" 2> "$work/server-backups.log"
bundle=$(python3 -I - "$PORCHLIGHT_ROOT/backups" "$work/server-backups.json" <<'PY'
import json,pathlib,sys
row=json.load(open(sys.argv[2]))['latest'];assert row
name=row['bundle_file'];assert name==pathlib.Path(name).name
bundle=pathlib.Path(sys.argv[1])/name;assert bundle.is_file()
print(bundle)
PY
)
# Expected checksum comes from the server's recorded successful run, not from hashing the same local file.
python3 -I - "$bundle" "$work/server-backups.json" "$work/bundle.sha256" <<'PY'
import json,pathlib,re,sys
bundle=pathlib.Path(sys.argv[1]);row=json.load(open(sys.argv[2]))['latest']
assert bundle.stat().st_size==row['size'] and re.fullmatch('[0-9a-f]{64}',row['sha256'])
pathlib.Path(sys.argv[3]).write_text(row['sha256']+'  '+str(bundle)+'\n')
PY
stage=restore
private bundle-sha256 sha256sum --check "$work/bundle.sha256"
drill_args=(); (( ! keep )) || drill_args+=(--keep)
private restore deploy/scripts/restore-drill.sh "$bundle" --source "$source" --identity "$identity" "${drill_args[@]}"
printf 'PASS independent backup restore drill\n'
stage=upgrade
private upgrade bash "$newer/install.sh" upgrade --source "$newer" "${args[@]}"
porch=$newer/porch-$new-x86_64-linux-musl
private post-version dc exec -T api /usr/local/bin/porchlight --version
python3 -I - "$work/post-version.log" "$PORCHLIGHT_ROOT/release-current/manifest.json" "$new" <<'PY'
import json,pathlib,sys
assert pathlib.Path(sys.argv[1]).read_text().strip().split()==['porchlight',sys.argv[3]]
assert json.load(open(sys.argv[2]))['version']==sys.argv[3]
PY
cli items show "$item" > "$work/post-item.json" 2> "$work/post.log"
cli lists show "$list" > "$work/post-list.json" 2> "$work/post-list.log"
python3 -I - "$work/post-list.json" "$list" "$item" <<'PY'
import json,sys
row=json.load(open(sys.argv[1]));assert row['id']==sys.argv[2] and row['item_id']==sys.argv[3] and row['name']=='M8 checklist'
assert any(entry['label']=='M8 entry' for entry in row['entries'])
PY
cli comments list "$item" > "$work/post-comments.json" 2>> "$work/post.log"
private post-export cli export --household --out "$work/post-household.tar.gz"
private post-complete python3 -I deploy/lib/verify-export.py "$work/post-household.tar.gz"
python3 -I - "$work/post-item.json" "$work/post-comments.json" <<'PY'
import json,sys
assert json.load(open(sys.argv[1]))['title']=='M8 acceptance item'
assert any(row['body']=='M8 comment' for row in json.load(open(sys.argv[2]))['items'])
PY
# Verify attachment bytes over enrolled mTLS; bearer header read from a private file, not ps.
python3 -I - "$work/devices/desktop/session.token" "$work/auth.header" <<'PY'
import os,pathlib,sys
with open(os.open(sys.argv[2],os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600),'w') as f: f.write('Authorization: Bearer '+pathlib.Path(sys.argv[1]).read_text().strip()+'\n')
PY
curl --silent --show-error --fail --cacert "$work/devices/desktop/ca.crt" --cert "$work/devices/desktop/client.crt" --key "$work/devices/desktop/client.key" \
  -H "@$work/auth.header" -H "Porchlight-Client-Version: $new" "https://localhost:$LAN_PORT/v1/attachments/$attachment" > "$work/post-attachment.txt" 2> "$work/curl.log"
cmp "$work/attachment.txt" "$work/post-attachment.txt"
printf 'PASS newer upgrade and preserved data/blob/export\n'
stage=desktop
mkdir -m 700 "$work/home"
private desktop env HOME="$work/home" XDG_DATA_HOME="$work/home/.local/share" XDG_CONFIG_HOME="$work/home/.config" XDG_STATE_HOME="$work/home/.local/state" bash "$newer/install.sh" desktop --source "$newer"
app=$work/home/.local/share/porchlight/current/app/bin/porchlight
[[ -x $app ]] || fail 'desktop bundled app launcher missing'
# Launcher JVM user.home must also be scratch; normal wallet and device.p12 are never touched.
private pull env HOME="$work/home" JAVA_TOOL_OPTIONS="-Duser.home=$work/home" "$app" --backup-pull-smoke "$work/devices/desktop" "https://localhost:$LAN_PORT"
python3 -I - "$work/home/.local/share/porchlight/backups" "$PORCHLIGHT_ROOT/backups" <<'PY'
import hashlib,pathlib,sys
files=list(pathlib.Path(sys.argv[1]).rglob('*.tar.age'));assert len(files)==1
def digest(path):
 h=hashlib.sha256()
 with path.open('rb') as f:
  while block:=f.read(65536): h.update(block)
 return h.hexdigest()
assert files[0].stat().st_size>0
assert digest(files[0]) in {digest(p) for p in pathlib.Path(sys.argv[2]).glob('porchlight-*.tar.age')}
PY
printf 'PASS desktop scratch HOME install and production passive pull\n'
stage=apk
reap_stale_netsimd
pgrep -u "$(id -u)" -x netsimd >/dev/null && fail 'stale netsimd did not exit'
emulator -avd porch-api35 -port 5580 -no-window -no-audio -no-snapshot -gpu swiftshader_indirect > "$work/emulator.log" 2>&1 &
emulator_pid=$!
ready=0
for ((i=0; i<120; i++)); do
  kill -0 "$emulator_pid" 2>/dev/null || fail 'porch-api35 emulator exited'
  if [[ $(adb -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r') == 1 ]]; then ready=1; break; fi
  sleep 2
done
((ready)) || fail 'porch-api35 boot timeout'
[[ $(adb -s "$serial" shell getprop ro.build.version.sdk | tr -d '\r') == 35 ]] || fail 'wrong emulator API'
apksigner=$ANDROID_HOME/build-tools/$(python3 -I - "$ANDROID_HOME/build-tools" <<'PY'
import pathlib,sys
print(sorted((p.name for p in pathlib.Path(sys.argv[1]).iterdir() if (p/'apksigner').is_file()),key=lambda s:tuple(int(x) for x in s.split('.') if x.isdigit()))[-1])
PY
)/apksigner
private apk-release-signature "$apksigner" verify --print-certs "$newer/porchlight-$new.apk"
# API35 is a development AVD: deliberately discard app data from debug/old-signature installs.
if adb -s "$serial" shell pm path app.porchlight > "$work/apk-before.log" 2>&1 && [[ -s $work/apk-before.log ]]; then
  private apk-uninstall adb -s "$serial" uninstall app.porchlight
fi
release_install_attempted=1
private apk adb -s "$serial" install -r "$newer/porchlight-$new.apk"
installed_path=$(adb -s "$serial" shell pm path app.porchlight | tr -d '\r')
[[ $installed_path == package:/* && $installed_path != *$'\n'* ]] || fail 'installed release APK path missing or ambiguous'
private apk-pull adb -s "$serial" pull "${installed_path#package:}" "$work/installed.apk"
private apk-installed-signature "$apksigner" verify --print-certs "$work/installed.apk"
python3 -I - "$work/apk-release-signature.log" "$work/apk-installed-signature.log" <<'PY'
import pathlib,re,sys
def signers(path):
 return set(re.findall(r'^Signer #\d+ certificate SHA-256 digest: ([0-9a-f]{64})$',pathlib.Path(path).read_text(),re.M))
expected=signers(sys.argv[1]);assert expected and expected==signers(sys.argv[2])
PY
printf 'PASS signed release APK installed on porch-api35\n'
