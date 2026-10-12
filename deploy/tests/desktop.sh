#!/usr/bin/env bash
# Fake minisign only here: controller must separately prove real signature verification.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$root/var/tests"
fixture=$(mktemp -d "$root/var/tests/desktop.XXXXXX")
trap 'rm -rf "$fixture"' EXIT
export HOME="$fixture/home with spaces" XDG_DATA_HOME="$fixture/data with spaces" XDG_CONFIG_HOME="$fixture/config with spaces" XDG_STATE_HOME="$fixture/state"
mkdir -p "$HOME" "$fixture/bin"
export PATH="$fixture/bin:$PATH"
cat > "$fixture/bin/minisign" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ ${REJECT_SIGNATURE:-0} == 0 ]]
SH
chmod 755 "$fixture/bin/minisign"
make_release() {
  local version=$1 dir=$fixture/release-$1
  mkdir -p "$dir" "$fixture/image/porchlight/bin"
  printf '#!/bin/sh\nexit 0\n' > "$fixture/image/porchlight/bin/porchlight"
  chmod 755 "$fixture/image/porchlight/bin/porchlight"
  mkdir -p "$fixture/image/porchlight/lib/runtime"
  printf runtime > "$fixture/image/porchlight/lib/runtime/real"
  ln -sf real "$fixture/image/porchlight/lib/runtime/link1"
  ln -sf real "$fixture/image/porchlight/lib/runtime/link2"
  tar --hard-dereference -chzf "$dir/porchlight-desktop-$version-linux-x86_64.tar.gz" -C "$fixture/image" porchlight
  python3 "$root/scripts/build-installer.py" "$root" "$dir/install.sh" TEST-PUBLIC-KEY
  cp -a "$root/deploy/lib" "$dir/lib"
  # Python bytecode is runtime state, never a release member.
  rm -rf "$dir/lib/__pycache__"
  cp "$root/design/icon/porchlight-icon.svg" "$dir/porchlight.svg"
  # Keep the oldest fixture descriptor-free to exercise legacy full-dist verification.
  if [[ $version != 0.1.0 ]]; then
    python3 -I -B - "$dir" "$version" <<'PY_RELEASE'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]);v=sys.argv[2];major,minor,patch=map(int,v.split('.'))
archive=f'porchlight-desktop-{v}-linux-x86_64.tar.gz'
files=sorted(['install.sh','porchlight.svg','release.json',archive]+[x.relative_to(p).as_posix() for x in (p/'lib').rglob('*') if x.is_file()])
record={'format_version':1,'version':v,'commit':None,'dirty':True,'source':{'github':'example-owner/porchlight'},
        'android':{'apk':f'porchlight-{v}.apk','package':'app.porchlight','version_code':major*10000+minor*100+patch,'signer_sha256':['a'*64]},
        'desktop':{'archive':archive,'files':files}}
(p/'release.json').write_text(json.dumps(record,indent=1,sort_keys=True)+'\n')
(p/f'porchlight-api-{v}.tar').write_text('server-only fixture')
PY_RELEASE
  fi
  python3 "$root/deploy/lib/operations.py" manifest "$dir" "$version"
  printf fake-signature > "$dir/manifest.json.minisig"
}
make_release 0.1.0
# Friendly symlink refusals, invalid first extraction and failed first activation must leave niri unchanged.
(
  # shellcheck disable=SC2030 # Each subshell intentionally isolates its own XDG fixture overrides.
  export XDG_DATA_HOME="$fixture/invalid-data" XDG_CONFIG_HOME="$fixture/invalid-config" XDG_STATE_HOME="$fixture/invalid-state"
  mkdir -p "$XDG_CONFIG_HOME/niri"
  printf 'keep invalid first install\n' > "$XDG_CONFIG_HOME/niri/config.kdl"
  cp "$XDG_CONFIG_HOME/niri/config.kdl" "$fixture/invalid-original"
  cp -a "$fixture/release-0.1.0" "$fixture/invalid-release"
  printf 'not an archive' > "$fixture/invalid-release/porchlight-desktop-0.1.0-linux-x86_64.tar.gz"
  python3 "$root/deploy/lib/operations.py" manifest "$fixture/invalid-release" 0.1.0
  if bash "$fixture/invalid-release/install.sh" desktop --source "$fixture/invalid-release" --niri > "$fixture/invalid.log" 2>&1; then exit 1; fi
  [[ $(< "$fixture/invalid.log") == *'operations: validation or I/O failed'* ]]
  cmp "$fixture/invalid-original" "$XDG_CONFIG_HOME/niri/config.kdl"
  [[ ! -e $XDG_DATA_HOME/porchlight/current && ! -e $XDG_CONFIG_HOME/autostart/porchlight.desktop ]]
)
(
  # shellcheck disable=SC2030,SC2031 # Each subshell intentionally isolates its own XDG fixture overrides.
  export XDG_DATA_HOME="$fixture/first-data" XDG_CONFIG_HOME="$fixture/first-config" XDG_STATE_HOME="$fixture/first-state"
  mkdir -p "$XDG_CONFIG_HOME/niri"
  printf 'original niri\n' > "$fixture/first-original"
  ln -s "$fixture/first-original" "$XDG_CONFIG_HOME/niri/config.kdl"
  if bash "$fixture/release-0.1.0/install.sh" desktop --source "$fixture/release-0.1.0" --niri > "$fixture/niri.log" 2>&1; then exit 1; fi
  [[ $(< "$fixture/niri.log") == *'Cannot edit niri config safely'* && $(< "$fixture/niri.log") != *Traceback* ]]
  rm "$XDG_CONFIG_HOME/niri/config.kdl"
  rmdir "$XDG_CONFIG_HOME/niri"
  mkdir "$fixture/niri-real"
  ln -s "$fixture/niri-real" "$XDG_CONFIG_HOME/niri"
  if bash "$fixture/release-0.1.0/install.sh" desktop --source "$fixture/release-0.1.0" --niri > "$fixture/niri.log" 2>&1; then exit 1; fi
  [[ $(< "$fixture/niri.log") == *'Cannot edit niri config safely'* && $(< "$fixture/niri.log") != *Traceback* ]]
  rm "$XDG_CONFIG_HOME/niri"; mkdir "$XDG_CONFIG_HOME/niri"
  cp "$fixture/first-original" "$XDG_CONFIG_HOME/niri/config.kdl"
  cat > "$fixture/bin/mv" <<'SH_MV'
#!/bin/bash
for arg in "$@"; do
  if [[ $arg == "$XDG_DATA_HOME/porchlight/current" ]]; then exit 1; fi
done
exec /usr/bin/mv "$@"
SH_MV
  chmod 755 "$fixture/bin/mv"
  if bash "$fixture/release-0.1.0/install.sh" desktop --source "$fixture/release-0.1.0" --niri > "$fixture/niri.log" 2>&1; then exit 1; fi
  cmp "$fixture/first-original" "$XDG_CONFIG_HOME/niri/config.kdl"
  [[ ! -e $XDG_DATA_HOME/porchlight/current && ! -e $XDG_CONFIG_HOME/autostart/porchlight.desktop ]]
  rm "$fixture/bin/mv"
)
printf 'PASS friendly niri symlink refusal and failed first install autostart cleanup\n'
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
mkdir -p "$XDG_CONFIG_HOME/niri"
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
printf ' // keep trailing spaces  \n' > "$XDG_CONFIG_HOME/niri/config.kdl"
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
cp "$XDG_CONFIG_HOME/niri/config.kdl" "$fixture/original.kdl"
bash "$fixture/release-0.1.0/install.sh" desktop --source "$fixture/release-0.1.0" --niri
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
[[ $(readlink "$XDG_DATA_HOME/porchlight/current") == releases/0.1.0 ]]
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
[[ -x $XDG_DATA_HOME/porchlight/current/app/bin/porchlight ]]
bash "$fixture/release-0.1.0/install.sh" desktop --source "$fixture/release-0.1.0" --niri
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
[[ $(wc -l < "$XDG_CONFIG_HOME/niri/config.kdl") == 2 ]]
# Existing minimized startup is the same mechanism; reinstall must not append a tray instance.
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
python3 -I - "$XDG_CONFIG_HOME/niri/config.kdl" <<'PY'
from pathlib import Path
import sys
p = Path(sys.argv[1]); p.write_text(p.read_text().replace('"--tray"', '"--minimized"'))
PY
bash "$fixture/release-0.1.0/install.sh" desktop --source "$fixture/release-0.1.0" --niri
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
[[ $(wc -l < "$XDG_CONFIG_HOME/niri/config.kdl") == 2 ]]
printf 'PASS niri existing minimized startup deduplicated\n'

# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
[[ ! -e $XDG_CONFIG_HOME/autostart/porchlight.desktop ]] # one mechanism: explicit niri, no installer XDG entry
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
cmp "$fixture/original.kdl" "$XDG_CONFIG_HOME/niri/config.kdl.bak"
make_release 0.2.0
python3 -I -B - "$fixture" <<'PY_SUBSET'
import json,pathlib,shutil,sys
p=pathlib.Path(sys.argv[1]);source=p/'release-0.2.0';subset=p/'subset'
for name in json.loads((source/'release.json').read_text())['desktop']['files']+['manifest.json','manifest.json.minisig']:
 target=subset/name;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(source/name,target)
PY_SUBSET
cp -a "$fixture/subset" "$fixture/missing-subset"
rm "$fixture/missing-subset/porchlight.svg"
if "$HOME/.local/bin/porchlight-update" "$fixture/missing-subset" > "$fixture/subset.log" 2>&1; then exit 1; fi
cp -a "$fixture/subset" "$fixture/extra-subset"
printf 'unlisted' > "$fixture/extra-subset/extra"
if "$HOME/.local/bin/porchlight-update" "$fixture/extra-subset" > "$fixture/subset.log" 2>&1; then exit 1; fi
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
[[ $(readlink "$XDG_DATA_HOME/porchlight/current") == releases/0.1.0 ]]
cat > "$fixture/bin/rsync" <<'SH_RSYNC'
#!/usr/bin/env bash
set -euo pipefail
[[ $* == *'--no-links --protect-args -- stackuser@release-host:/release/new/'* ]]
cp -a "$DESKTOP_REMOTE_FIXTURE/." "${@: -1}"
SH_RSYNC
chmod 755 "$fixture/bin/rsync"
DESKTOP_REMOTE_FIXTURE="$fixture/subset" "$HOME/.local/bin/porchlight-update" stackuser@release-host:/release/new
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
[[ $(readlink "$XDG_DATA_HOME/porchlight/current") == releases/0.2.0 ]]
status=0
"$HOME/.local/bin/porchlight-update" "$fixture/release-0.1.0" > "$fixture/downgrade.log" 2>&1 || status=$?
[[ $status == 2 && $(< "$fixture/downgrade.log") == *'refusing downgrade; use porchlight-update --rollback'* ]]
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
python3 -I -B - "$XDG_DATA_HOME/porchlight/current" "$fixture/current-before" <<'PY_CURRENT'
import pathlib,sys
p=pathlib.Path(sys.argv[1]);pathlib.Path(sys.argv[2]).write_text(str(p.lstat().st_mtime_ns))
PY_CURRENT
"$HOME/.local/bin/porchlight-update" "$fixture/subset" > "$fixture/same.log"
[[ $(< "$fixture/same.log") == *'PASS already installed 0.2.0'* ]]
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
python3 -I -B - "$XDG_DATA_HOME/porchlight/current" "$fixture/current-before" "$fixture" <<'PY_CURRENT'
import pathlib,sys
assert str(pathlib.Path(sys.argv[1]).lstat().st_mtime_ns)==pathlib.Path(sys.argv[2]).read_text()
assert not list(pathlib.Path(sys.argv[3]).rglob('__pycache__'))
PY_CURRENT
printf 'PASS desktop subset, missing/extra refusal, downgrade guard and same-version no-op\n'
# Reinstall the current release must not erase rollback history.
bash "$fixture/release-0.2.0/install.sh" desktop --source "$fixture/release-0.2.0"
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
[[ $(readlink "$XDG_DATA_HOME/porchlight/previous") == releases/0.1.0 ]]
"$HOME/.local/bin/porchlight-update" --rollback
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
[[ $(readlink "$XDG_DATA_HOME/porchlight/current") == releases/0.1.0 ]]
if REJECT_SIGNATURE=1 "$HOME/.local/bin/porchlight-update" "$fixture/release-0.2.0" >/dev/null 2>&1; then exit 1; fi
# A malicious sibling lib must never be sourced/executed, even by a standalone copied installer.
cp "$fixture/release-0.1.0/install.sh" "$fixture/trusted-install.sh"
printf '\nprintf compromised > "%s"\n' "$fixture/compromised" >> "$fixture/release-0.2.0/lib/operations.sh"
printf '\nopen(%s,"w").write("compromised")\n' "'${fixture}/compromised'" >> "$fixture/release-0.2.0/lib/operations.py"
if bash "$fixture/trusted-install.sh" desktop --source "$fixture/release-0.2.0" > "$fixture/tamper.log" 2>&1; then exit 1; fi
[[ ! -e $fixture/compromised && $(< "$fixture/tamper.log") == *'operations: validation or I/O failed'* ]]
if "$HOME/.local/bin/porchlight-update" "$fixture/release-0.2.0" >/dev/null 2>&1; then exit 1; fi
[[ ! -e $fixture/compromised ]]
printf corrupt >> "$fixture/release-0.2.0/porchlight.svg"
if "$HOME/.local/bin/porchlight-update" "$fixture/release-0.2.0" >/dev/null 2>&1; then exit 1; fi
# shellcheck disable=SC2031 # Use parent XDG paths; the earlier fixture overrides are intentionally subshell-only.
[[ $(readlink "$XDG_DATA_HOME/porchlight/current") == releases/0.1.0 ]]
printf 'PASS desktop install, XDG paths, update, rollback, tamper rejection\n'
# Server/restore guards must detect a running Compose project even when a different target root has no metadata.
cat > "$fixture/bin/docker" <<'SH'
#!/usr/bin/env bash
case $1 in
  info) exit 0 ;;
  compose) [[ ${2:-} == version ]] ;;
  ps) printf 'existing-project-container\n' ;;
  *) exit 77 ;;
esac
SH
cat > "$fixture/bin/age" <<'PY'
#!/usr/bin/env python3
import shutil,sys
shutil.copyfile(sys.argv[-1],sys.argv[sys.argv.index('-o')+1])
PY
chmod 755 "$fixture/bin/docker" "$fixture/bin/age"
if bash "$fixture/release-0.1.0/install.sh" server --root "$fixture/guard-server" --run-dir "$fixture/run" > "$fixture/guard.log" 2>&1; then exit 1; fi
[[ $(< "$fixture/guard.log") == *'Compose project already exists'* ]]
mkdir -p "$fixture/bundle/pki" "$fixture/bundle/secrets" "$fixture/bundle/blobs"
for member in db.dump .env pki/ca.crt pki/ca.key secrets/idempotency.key backup-recipient.txt; do printf fixture > "$fixture/bundle/$member"; done
printf fixture > "$fixture/identity"
python3 "$root/deploy/lib/operations.py" manifest "$fixture/bundle" 0.1.0
tar -cf "$fixture/bundle.tar.age" -C "$fixture/bundle" .
if bash "$fixture/release-0.1.0/install.sh" restore "$fixture/bundle.tar.age" --identity "$fixture/identity" --root "$fixture/guard-restore" --run-dir "$fixture/run" > "$fixture/guard.log" 2>&1; then exit 1; fi
[[ $(< "$fixture/guard.log") == *'existing stack: use --force'* ]]
if bash "$fixture/release-0.1.0/install.sh" restore "$fixture/bundle.tar.age" --identity "$fixture/identity" --root "$fixture/guard-restore" --run-dir "$fixture/run" --force > "$fixture/guard.log" 2>&1; then exit 1; fi
[[ $(< "$fixture/guard.log") == *'existing project belongs to another/missing root'* ]]
[[ ! -e $fixture/guard-restore/.env && ! -e $fixture/guard-server/.env ]]
printf 'PASS server/restore guard running projects outside target root\n'

# Stopped stacks are destructive targets too; a valid new root --no-start restores without starting writers.
cat > "$fixture/bin/docker" <<'PY_DOCKER'
#!/usr/bin/env python3
import os,pathlib,sys
args=sys.argv[1:]
with open(os.environ['O1_DOCKER_LOG'],'a') as f: f.write(' '.join(args)+'\n')
if args[0]=='ps' and os.environ.get('O1_PROJECT_PRESENT'): print('stopped-container')
PY_DOCKER
chmod 755 "$fixture/bin/docker"
export O1_DOCKER_LOG="$fixture/docker.log"
# Malformed IPv6 used to fail only in process substitution and silently yield no SANs.
printf 'fixture orchard river\n' > "$fixture/admin.pass"
printf -v recipient 'age1%058d' 0
if ENROLL_CIDRS=127.0.0.1/32 PORCHLIGHT_ENDPOINTS='https://[broken' bash "$fixture/trusted-install.sh" server --source "$fixture/release-0.1.0" --root "$fixture/bad-sans" --run-dir "$fixture/run" --backup-recipient "$recipient" --admin-name Fixture --admin-password-file "$fixture/admin.pass" > "$fixture/sans.log" 2>&1; then exit 1; fi
[[ $(< "$fixture/sans.log") == *'Cannot derive certificate SANs'* && $(< "$fixture/sans.log") != *Traceback* ]]
[[ $(< "$fixture/docker.log") != *'load -i'* && $(< "$fixture/docker.log") != *'--exit-code-from migrate'* ]]
printf 'PASS SAN derivation failure refuses image load and migration\n'
mkdir -p "$fixture/stopped"
printf 'old fixture env\n' > "$fixture/stopped/.env"
if bash "$fixture/trusted-install.sh" restore "$fixture/bundle.tar.age" --source "$fixture/release-0.1.0" --identity "$fixture/identity" --root "$fixture/stopped" --run-dir "$fixture/run" > "$fixture/guard.log" 2>&1; then exit 1; fi
[[ $(< "$fixture/guard.log") == *'existing stack: use --force'* ]]
export O1_PROJECT_PRESENT=1
if bash "$fixture/trusted-install.sh" restore "$fixture/bundle.tar.age" --source "$fixture/release-0.1.0" --identity "$fixture/identity" --root "$fixture/unknown-project" --run-dir "$fixture/run" --force > "$fixture/guard.log" 2>&1; then exit 1; fi
[[ $(< "$fixture/guard.log") == *'existing project belongs'* ]]
unset O1_PROJECT_PRESENT
: > "$fixture/docker.log"
bash "$fixture/trusted-install.sh" restore "$fixture/bundle.tar.age" --source "$fixture/release-0.1.0" --identity "$fixture/identity" --root "$fixture/stopped" --run-dir "$fixture/run" --force --no-start
python3 - "$fixture" <<'PY_RESTORE'
from pathlib import Path
import sys
p=Path(sys.argv[1]);log=(p/'docker.log').read_text()
assert log.index('dropdb') < log.index('/docker-entrypoint-initdb.d/10-porchlight.sh') < log.index('pg_restore') < log.index('--exit-code-from migrate')
assert 'createdb' not in log and 'ALTER DATABASE' not in log
assert 'SELECT version, success FROM public._sqlx_migrations' in log
assert '--wait api' not in log and 'healthcheck' not in log and '--wait backup' not in log
assert next((p/'stopped').glob('.env.before-restore.*')).read_text()=='old fixture env\n'
# Drill contract is ordered: no-start restore, counts/CA/migration checks, API start, health.
PY_RESTORE
python3 - "$root" <<'PY_DRILL'
from pathlib import Path
import sys
s=Path(sys.argv[1],'deploy/scripts/restore-drill.sh').read_text()
assert s.index('--no-start') < s.index('table_row_counts') < s.index('dc up -d --wait api') < s.index('dc exec -T api')
PY_DRILL
printf 'PASS stopped-stack guard, displaced env, shared bootstrap ordering and restore --no-start/drill sequencing\n'
