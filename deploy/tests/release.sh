#!/usr/bin/env bash
# Entire toolchain mocked in a private tree. No real keys/Docker/Gradle/signatures are created.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$root/var/tests"
fixture=$(mktemp -d "$root/var/tests/release.XXXXXX")
trap 'rm -rf "$fixture"' EXIT
export PYTHONDONTWRITEBYTECODE=1 BUILD_LOG=$fixture/build.log
mkdir -p "$fixture/scripts/bin" "$fixture/deploy/lib" "$fixture/java/bin" "$fixture/sdk/build-tools/1.0.0" "$fixture/client" "$fixture/server/db/migrations" "$fixture/design/icon"
cp "$root/scripts/release.sh" "$root/scripts/release-keys.sh" "$root/scripts/build-installer.py" "$fixture/scripts/"
cp -a "$root/deploy/lib/." "$fixture/deploy/lib/"
cp "$root/deploy/install.sh" "$root/deploy/compose.yml" "$root/deploy/compose.funnel.yml" "$root/deploy/env.example" "$fixture/deploy/"
cp -a "$root/deploy/postgres" "$fixture/deploy/"
cp "$root/design/icon/porchlight-icon.svg" "$fixture/design/icon/"
printf '0.1.0\n' > "$fixture/VERSION"
printf 'SELECT 1;\n' > "$fixture/server/db/migrations/0001_fixture.sql"
cat > "$fixture/scripts/env.sh" <<'SH'
export JAVA_HOME="$root/java" ANDROID_HOME="$root/sdk"
export PATH="$root/scripts/bin:$PATH"
SH
cat > "$fixture/scripts/bin/minisign" <<'PY'
#!/usr/bin/env python3
import base64,pathlib,sys
args=sys.argv[1:]
def arg(n): return pathlib.Path(args[args.index(n)+1])
if '-G' in args:
 arg('-s').write_text('NOT A PRIVATE KEY\n');arg('-p').write_text('untrusted comment: fixture\n'+base64.b64encode(bytes(42)).decode()+'\n')
elif '-Sm' in args: arg('-x').write_text('FAKE SIGNATURE\n')
elif '-Vm' in args:
 p=arg('-x') if '-x' in args else pathlib.Path(str(arg('-Vm'))+'.minisig')
 assert p.read_text()=='FAKE SIGNATURE\n'
else: sys.exit(1)
PY
cat > "$fixture/scripts/bin/openssl" <<'SH'
#!/usr/bin/env bash
printf '%064d\n' 1
SH
cat > "$fixture/java/bin/keytool" <<'PY'
#!/usr/bin/env python3
import pathlib,sys
pathlib.Path(sys.argv[sys.argv.index('-keystore')+1]).write_bytes(b'NOT A KEYSTORE')
assert '-storepass:file' in sys.argv and '-keypass:file' in sys.argv
PY
cat > "$fixture/scripts/bin/docker" <<'PY'
#!/usr/bin/env python3
import os,pathlib,sys
with open(os.environ['BUILD_LOG'],'a') as log: log.write('docker\n')
args=sys.argv[1:]
if args[0]=='build' and '--output' in args:
 out=args[args.index('--output')+1].split('dest=')[1];p=pathlib.Path(out);p.mkdir(parents=True);(p/'porch').write_text('fake static porch')
elif args[0]=='save': pathlib.Path(args[args.index('-o')+1]).write_bytes(b'fake image tar')
PY
cat > "$fixture/client/gradlew" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
mkdir -p androidApp/build/outputs/apk/release desktopApp/build/compose/binaries/main/app/porchlight/bin desktopApp/build/compose/binaries/main/app/porchlight/lib/runtime
printf 'not a signed APK' > androidApp/build/outputs/apk/release/androidApp-release.apk
printf '#!/bin/sh\nexit 0\n' > desktopApp/build/compose/binaries/main/app/porchlight/bin/porchlight
chmod 755 desktopApp/build/compose/binaries/main/app/porchlight/bin/porchlight
printf 'fake runtime' > desktopApp/build/compose/binaries/main/app/porchlight/lib/runtime/fixture
ln -sf fixture desktopApp/build/compose/binaries/main/app/porchlight/lib/runtime/link1
ln -sf fixture desktopApp/build/compose/binaries/main/app/porchlight/lib/runtime/link2
SH
cat > "$fixture/scripts/bin/git" <<'SH'
#!/usr/bin/env bash
# Never fall through to real git, including in the parent source tree.
[[ ${MOCK_GIT:-missing} != missing ]] || exit 1
case $3 in
  rev-parse) printf 'a%.0s' {1..40}; printf '\n' ;;
  status)
    [[ ${MOCK_GIT:-} != failed-status ]] || exit 1
    if [[ ${MOCK_GIT:-} == dirty ]]; then printf ' M VERSION\n'; fi ;;
  *) exit 1 ;;
esac
SH
cat > "$fixture/sdk/build-tools/1.0.0/apksigner" <<'SH'
#!/usr/bin/env bash
[[ ${FAIL_APK_SIGNATURE:-0} == 0 ]] || exit 1
[[ $1 == verify && $2 == --print-certs ]] || exit 1
IFS=, read -r -a signers <<< "${APK_SIGNERS:-$(printf '%064d' 1)}"
i=1
for signer in "${signers[@]}"; do
  printf 'Signer #%s certificate SHA-256 digest: %s\n' "$i" "$signer"
  i=$((i+1))
done
SH
chmod 755 "$fixture/scripts/"*.sh "$fixture/scripts/bin/"* "$fixture/java/bin/keytool" "$fixture/sdk/build-tools/1.0.0/apksigner" "$fixture/client/gradlew"
bash "$fixture/scripts/release-keys.sh" > "$fixture/key-paths.log"
[[ $(stat -c %a "$fixture/var/release") == 700 ]]
[[ $(stat -c %a "$fixture/var/release/keystore.pass") == 600 ]]
if bash "$fixture/scripts/release-keys.sh" >/dev/null 2>&1; then exit 1; fi
mkdir -p "$fixture/deploy/release-keys" "$fixture/deploy/lib/__pycache__"
cp "$fixture/var/release/minisign.pub" "$fixture/deploy/release-keys/minisign.pub"
printf '# Fixture signer\n\n%064d\n' 1 > "$fixture/deploy/release-keys/android-signer.sha256"
printf 'not bytecode\n' > "$fixture/deploy/lib/__pycache__/operations.pyc"
printf 'not bytecode\n' > "$fixture/deploy/lib/stray.pyc"
export PORCHLIGHT_RELEASE_REPO=example-owner/porchlight
bash "$fixture/scripts/release.sh"
python3 "$root/deploy/lib/operations.py" verify "$fixture/dist/0.1.0"
python3 -I - "$fixture/dist/0.1.0" <<'PY'
import json,pathlib,sys
root=pathlib.Path(sys.argv[1]); text=(root/'release.json').read_text(); release=json.loads(text)
assert text==json.dumps(release,indent=1,sort_keys=True)+'\n'
assert release['version']=='0.1.0' and release['format_version']==1
assert release['commit'] is None and release['dirty'] is True
assert release['source']=={'github':'example-owner/porchlight'}
assert release['android']=={'apk':'porchlight-0.1.0.apk','package':'app.porchlight','version_code':100,'signer_sha256':['0'*63+'1']}
archive='porchlight-desktop-0.1.0-linux-x86_64.tar.gz'
files=['install.sh','porchlight.svg',archive,'release.json']+[p.relative_to(root).as_posix() for p in (root/'lib').rglob('*') if p.is_file()]
assert release['desktop']=={'archive':archive,'files':sorted(files)}
manifest=json.loads((root/'manifest.json').read_text())['files']
assert all(name in manifest for name in files)
assert not any('__pycache__' in name or name.endswith('.pyc') for name in manifest)
PY
[[ -f $fixture/dist/0.1.0/porchlight-0.1.0.apk ]]
export HOME="$fixture/home" XDG_STATE_HOME="$fixture/state" XDG_DATA_HOME="$fixture/data" XDG_CONFIG_HOME="$fixture/config"
export PATH="$fixture/scripts/bin:$PATH"
bash "$fixture/dist/0.1.0/install.sh" desktop --source "$fixture/dist/0.1.0"
[[ -x $XDG_DATA_HOME/porchlight/current/app/bin/porchlight ]]
cmp "$XDG_DATA_HOME/porchlight/current/app/lib/runtime/fixture" "$XDG_DATA_HOME/porchlight/current/app/lib/runtime/link2"
if bash "$fixture/scripts/release.sh" >/dev/null 2>&1; then exit 1; fi
expect_refusal() {
  local code=$1 message=$2 status=0; shift 2
  "$@" > "$fixture/refusal.log" 2>&1 || status=$?
  [[ $status == "$code" ]] || { printf 'unexpected refusal status: %s (wanted %s)\n' "$status" "$code" >&2; exit 1; }
  grep -qF "$message" "$fixture/refusal.log"
}
expect_refusal 1 'APK signature verification failed' env FAIL_APK_SIGNATURE=1 bash "$fixture/scripts/release.sh" --force
printf '# Wrong signer\n%064d\n' 2 > "$fixture/deploy/release-keys/android-signer.sha256"
expect_refusal 1 'APK signer mismatch' bash "$fixture/scripts/release.sh" --force
printf '%064d\n' 1 > "$fixture/deploy/release-keys/android-signer.sha256"
before=$(wc -l < "$BUILD_LOG")
printf 'different key\n' >> "$fixture/deploy/release-keys/minisign.pub"
expect_refusal 2 'release key differs from deploy/release-keys/minisign.pub' bash "$fixture/scripts/release.sh" --force
cp "$fixture/var/release/minisign.pub" "$fixture/deploy/release-keys/minisign.pub"
expect_refusal 2 'invalid PORCHLIGHT_RELEASE_REPO' env PORCHLIGHT_RELEASE_REPO=bad/slug/extra bash "$fixture/scripts/release.sh" --force
[[ $(wc -l < "$BUILD_LOG") == "$before" ]]
PORCHLIGHT_RELEASE_REPO= bash "$fixture/scripts/release.sh" --force
python3 -I - "$fixture/dist/0.1.0/release.json" <<'PY'
import json,pathlib,sys
assert json.loads(pathlib.Path(sys.argv[1]).read_text())['source']['github'] is None
PY
for mode in clean dirty failed-status; do
  MOCK_GIT=$mode bash "$fixture/scripts/release.sh" --force > "$fixture/build-result.log"
  python3 -I - "$fixture/dist/0.1.0/release.json" "$mode" <<'PY'
import json,pathlib,sys
release=json.loads(pathlib.Path(sys.argv[1]).read_text());mode=sys.argv[2]
assert release['commit']==(None if mode=='failed-status' else 'a'*40)
assert release['dirty'] is (mode!='clean')
PY
done
printf '# Multiple signers, unordered/duplicated pin\n%064d\n%064d\n%064d\n' 2 1 1 > "$fixture/deploy/release-keys/android-signer.sha256"
APK_SIGNERS="$(printf '%064d' 2),$(printf '%064d' 1),$(printf '%064d' 2)" bash "$fixture/scripts/release.sh" --force > "$fixture/build-result.log"
python3 -I - "$fixture/dist/0.1.0/release.json" <<'PY'
import json,pathlib,sys
assert json.loads(pathlib.Path(sys.argv[1]).read_text())['android']['signer_sha256']==['0'*63+'1','0'*63+'2']
PY
printf 'SELECT 2;\n' >> "$fixture/server/db/migrations/0001_fixture.sql"
expect_refusal 1 'released migrations changed; refuse release' bash "$fixture/scripts/release.sh" --force
printf 'PASS key custody/refusal, signer pins, release metadata, bytecode exclusion, dirty dist and migration immutability\n'
