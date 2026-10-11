#!/usr/bin/env bash
# Controller-only, heavy, throwaway signed releases. Never publish these deliberately failing/debug-key builds.
set -euo pipefail
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd -P)
cd "$root"
[[ $root != */var/wt/* ]] || { printf 'FAIL run update gate build from the main tree\n'; exit 2; }
export PYTHONDONTWRITEBYTECODE=1 PORCHLIGHT_RELEASE_REPO=example-owner/porchlight
keep=0 android_only=0 discard=0
for arg in "$@"; do
  case $arg in --keep) keep=1 ;; --android-only) android_only=1 ;; --discard) discard=1 ;; *) printf 'usage: update-gate-build.sh [--keep] [--android-only] | --discard\n'; exit 2 ;; esac
done
(( ! discard || (! keep && ! android_only) )) || { printf 'FAIL --discard cannot be combined with build options\n'; exit 2; }
fail() { printf 'FAIL %s\n' "$1"; exit 1; }
state=$root/var/update-gate
[[ ! -L $state ]] || fail 'update gate state must not be a symlink'
mkdir -m 700 -p "$state"; chmod 700 "$state"
exec 9> "$state/.lock"
flock -n 9 || fail 'another update gate is running'
if ((discard)); then
  # Acceptance shares this lock. Also refuse survivors of an interrupted/kept acceptance.
  projects=$(docker ps -a --filter label=com.docker.compose.project --format '{{.Label "com.docker.compose.project"}}') || fail 'cannot check update gate containers'
  while IFS= read -r project; do
    [[ $project != porchlight-update-gate-* ]] || fail 'tear down retained update gate containers before discard'
  done <<< "$projects"
  # Only owned payloads; keep run/build logs, credentials needing diagnosis, and the lock inode.
  rm -rf -- "$state/dists" "$state/android" "$state/build.json"
  printf 'PASS discarded update gate artifacts (diagnostics retained)\n'
  exit 0
fi
. scripts/env.sh
copy=$root/var/wt/updgate-build
[[ ! -e $copy && ! -L $copy && ! -e $root/var/wt/.base/updgate-build ]] || fail 'inspect or drop the existing updgate-build copy first'
if ((android_only)); then
  # Rebuild only the 9.9.8 debug app/test APKs against the existing scratch releases (same debug signer).
  [[ -d $state/dists && -f $state/build.json ]] || fail 'android-only rebuild needs a complete earlier update gate build'
else
  [[ ! -e $state/dists && ! -e $state/android && ! -e $state/build.json ]] || fail 'inspect previous artifacts, then use --discard before rebuilding'
fi
[[ -s $HOME/.android/debug.keystore ]] || fail 'Android debug keystore required under the controller HOME'
work=$(mktemp -d "$state/build-$(date -u +%Y%m%dT%H%M%SZ).XXXXXXXX")
stage=create copy_owned=0
private() { local name=$1; shift; "$@" > "$work/$name.log" 2>&1; }
cleanup() {
  local code=$?
  trap - EXIT INT TERM
  if ((copy_owned && ! keep)); then
    scripts/wt.sh drop updgate-build > "$work/drop.log" 2>&1 || code=1
  fi
  if ((code)); then
    # An interrupted/failed build must never look complete to acceptance.
    rm -f -- "$state/build.json"
    printf 'FAIL update gate build %s (private diagnostics under var/update-gate)\n' "$stage"
  else
    printf 'PASS update gate build\n'
  fi
  exit "$code"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
copy_owned=1
private create scripts/wt.sh create updgate-build
build_android() {
  stage=android
  private bump-android python3 -I deploy/scripts/update-gate.py bump 9.9.8
  private android bash -c 'cd "$1"; . scripts/env.sh; cd client; ./gradlew --offline --max-workers=4 --no-configuration-cache --console=plain -Pkotlin.incremental=false :androidApp:assembleDebug :androidApp:assembleDebugAndroidTest' bash "$copy"
  rm -rf -- "$state/android"; mkdir -m 700 "$state/android"
  cp "$copy/client/androidApp/build/outputs/apk/debug/androidApp-debug.apk" "$state/android/"
  cp "$copy/client/androidApp/build/outputs/apk/androidTest/debug/androidApp-debug-androidTest.apk" "$state/android/"
  chmod 600 "$state/android/"*.apk
}
debug_signing() {
  # AGP's project-local ANDROID_USER_HOME can otherwise choose a different debug certificate.
  cp "$HOME/.android/debug.keystore" "$copy/var/android-user-home/debug.keystore"
  chmod 600 "$copy/var/android-user-home/debug.keystore"
  printf '\n// Update gate copy only: use the same debug signer as the scratch releases.\nandroid.signingConfigs.getByName("debug").storeFile = rootProject.file("../var/android-user-home/debug.keystore")\n' >> "$copy/client/androidApp/build.gradle.kts"
}
if ((android_only)); then
  debug_signing
  build_android
  stage=inventory
  python3 -I - "$state/build.json" "$work/minisign.pub" "$work/android-signer.sha256" <<'PY_RECORD'
import json,pathlib,sys
record=json.loads(pathlib.Path(sys.argv[1]).read_text())
pathlib.Path(sys.argv[2]).write_text('untrusted comment: update gate\n'+record['minisign_public_key']+'\n')
pathlib.Path(sys.argv[3]).write_text(record['debug_signer_sha256']+'\n')
PY_RECORD
  private inventory python3 -I deploy/scripts/update-gate.py record-build "$work/minisign.pub" "$work/android-signer.sha256"
  exit 0
fi
stage=keys
# Only the new copy creates release keys; the controller's signing directory is never read or written.
private keys bash -c 'cd "$1"; scripts/release-keys.sh' bash "$copy"
keys=$copy/var/release
printf 'android\n' > "$work/debug.pass"
rm -- "$keys/porchlight-release.jks"
private debug-import "$JAVA_HOME/bin/keytool" -importkeystore -noprompt \
  -srckeystore "$HOME/.android/debug.keystore" -srcalias androiddebugkey \
  -srcstorepass:file "$work/debug.pass" -srckeypass:file "$work/debug.pass" \
  -destkeystore "$keys/porchlight-release.jks" -deststoretype JKS -destalias porchlight \
  -deststorepass:file "$keys/keystore.pass" -destkeypass:file "$keys/keystore.pass"
chmod 600 "$keys/porchlight-release.jks"
private debug-export "$JAVA_HOME/bin/keytool" -exportcert -keystore "$keys/porchlight-release.jks" \
  -alias porchlight -storepass:file "$keys/keystore.pass" -file "$work/debug.der"
cp "$keys/minisign.pub" "$copy/deploy/release-keys/minisign.pub"
python3 -I - "$work/debug.der" "$copy/deploy/release-keys/android-signer.sha256" <<'PY'
import hashlib,pathlib,sys
pathlib.Path(sys.argv[2]).write_text(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest()+'\n')
PY
debug_signing
mkdir -m 700 "$state/dists" "$state/android"
for version in 9.9.8 9.9.9 9.9.10; do
  stage=build-$version
  private "bump-$version" python3 -I deploy/scripts/update-gate.py bump "$version"
  # release.sh sources this copy's env and builds/signs only the copy's dist.
  private "release-$version" bash -c 'cd "$1"; scripts/release.sh' bash "$copy"
  # Keep release modes: the scratch stack's postgres user reads init files; var/update-gate itself is 0700.
  cp -a "$copy/dist/$version" "$state/dists/$version"
  if [[ $version == 9.9.8 ]]; then
    private android bash -c 'cd "$1"; . scripts/env.sh; cd client; ./gradlew --offline --max-workers=4 --no-configuration-cache --console=plain -Pkotlin.incremental=false :androidApp:assembleDebug :androidApp:assembleDebugAndroidTest' bash "$copy"
    cp "$copy/client/androidApp/build/outputs/apk/debug/androidApp-debug.apk" "$state/android/"
    cp "$copy/client/androidApp/build/outputs/apk/androidTest/debug/androidApp-debug-androidTest.apk" "$state/android/"
    chmod 600 "$state/android/"*.apk
    apksigner=$(python3 -I - "$ANDROID_HOME/build-tools" <<'PY'
import pathlib,sys
paths=[p/'apksigner' for p in pathlib.Path(sys.argv[1]).iterdir() if (p/'apksigner').is_file()]
print(sorted(paths,key=lambda p:tuple(int(x) for x in p.parent.name.split('.') if x.isdigit()))[-1])
PY
)
    for apk in androidApp-debug androidApp-debug-androidTest; do
      private "$apk-signer" "$apksigner" verify --print-certs "$state/android/$apk.apk"
      python3 -I - "$work/$apk-signer.log" "$copy/deploy/release-keys/android-signer.sha256" <<'PY'
import pathlib,re,sys
signers=set(re.findall(r'^Signer #\d+ certificate SHA-256 digest: ([0-9a-f]{64})$',pathlib.Path(sys.argv[1]).read_text(),re.M))
assert signers=={pathlib.Path(sys.argv[2]).read_text().strip()}
PY
    done
  fi
  printf 'PASS scratch release %s built\n' "$version"
done
stage=inventory
private inventory python3 -I deploy/scripts/update-gate.py record-build "$keys/minisign.pub" "$copy/deploy/release-keys/android-signer.sha256"
