#!/usr/bin/env bash
# Build a local signed release for optional owner-run publishing. Docker builds are operator gates.
set -euo pipefail
umask 022
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
. scripts/env.sh
export CARGO_NET_OFFLINE=true PYTHONDONTWRITEBYTECODE=1
. deploy/lib/operations.sh
version=$(tr -d '\n\r' < VERSION)
code=$(version_code "$version")
force=0; case ${1:-} in '') ;; --force) force=1 ;; *) printf 'usage: scripts/release.sh [--force]\n' >&2; exit 2 ;; esac
out=$root/dist/$version
keys=$root/var/release
for name in minisign.key minisign.pub porchlight-release.jks keystore.pass; do
  [[ -s $keys/$name ]] || { printf 'run scripts/release-keys.sh first\n' >&2; exit 2; }
done
python3 -I - "$keys/minisign.pub" "$root/deploy/release-keys/minisign.pub" <<'PY'
import os,pathlib,re,sys
try:
 same=pathlib.Path(sys.argv[1]).read_text().splitlines()[-1]==pathlib.Path(sys.argv[2]).read_text().splitlines()[-1]
except (OSError,IndexError,UnicodeError): same=False
if not same:
 print('release key differs from deploy/release-keys/minisign.pub',file=sys.stderr);sys.exit(2)
repo=os.environ.get('PORCHLIGHT_RELEASE_REPO','')
if repo and not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]{0,38}/[A-Za-z0-9._-]{1,100}',repo):
 print('invalid PORCHLIGHT_RELEASE_REPO',file=sys.stderr);sys.exit(2)
PY
# Released SQL is immutable, including when --force rebuilds the same version.
python3 - "$root" "$keys/minisign.pub" <<'PY'
import hashlib,json,pathlib,subprocess,sys
root=pathlib.Path(sys.argv[1]);public=pathlib.Path(sys.argv[2]).read_text().splitlines()[-1]
sys.path.insert(0,str(root/'deploy/lib'));import operations
current={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in (root/'server/db/migrations').glob('*.sql')}
for release in (root/'dist').glob('*'):
 if not (release/'manifest.json.minisig').is_file(): continue
 subprocess.run(['minisign','-Vm',str(release/'manifest.json'),'-P',public],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True)
 operations.verify(release)
 if (release/'migrations.json').is_file():
  previous=json.loads((release/'migrations.json').read_text())
  if any(current.get(name)!=sha for name,sha in previous.items()): raise SystemExit('released migrations changed; refuse release')
PY
if [[ -e $out ]]; then
  ((force)) || { printf 'refusing dirty dist/%s; use --force\n' "$version" >&2; exit 2; }
  rm -rf -- "$out"
fi
mkdir -p "$out" "$root/var/release-work"
work=$(mktemp -d "$root/var/release-work/build.XXXXXX")
trap 'rm -rf "$work"' EXIT
public=$(python3 - "$keys/minisign.pub" <<'PY'
import base64,pathlib,sys
s=pathlib.Path(sys.argv[1]).read_text().splitlines()[-1];assert len(base64.b64decode(s,validate=True))==42;print(s)
PY
)
docker build -f deploy/Dockerfile --target runtime -t "porchlight-api:$version" server
docker build -f deploy/Dockerfile --target cli-export --output "type=local,dest=$work/cli" server
docker build -f deploy/backup/Dockerfile -t "porchlight-backup:$version" deploy
docker save -o "$out/porchlight-api-$version.tar" "porchlight-api:$version"
docker save -o "$out/porchlight-backup-$version.tar" "porchlight-backup:$version"
cp "$work/cli/porch" "$out/porch-$version-x86_64-linux-musl"
export PORCHLIGHT_KEYSTORE=$keys/porchlight-release.jks PORCHLIGHT_KEYSTORE_PASSWORD_FILE=$keys/keystore.pass
(cd client && ./gradlew --offline --max-workers=4 --no-configuration-cache --console=plain :androidApp:assembleRelease :desktopApp:createDistributable)
apk=$root/client/androidApp/build/outputs/apk/release/androidApp-release.apk
[[ -f $apk ]] || { printf 'signed APK missing; refusing unsigned release\n' >&2; exit 1; }
# Verify signing without passing the password anywhere.
if ! "$ANDROID_HOME/build-tools/$(python3 -I - "$ANDROID_HOME/build-tools" <<'PY'
import pathlib,sys
print(sorted((p.name for p in pathlib.Path(sys.argv[1]).iterdir() if (p/'apksigner').is_file()),key=lambda s:tuple(int(x) for x in s.split('.') if x.isdigit()))[-1])
PY
)/apksigner" verify --print-certs "$apk" > "$work/apk-certs" 2>/dev/null; then
  printf 'APK signature verification failed\n' >&2; exit 1
fi
python3 -I - "$work/apk-certs" "$root/deploy/release-keys/android-signer.sha256" "$work/signers.json" <<'PY'
import json,pathlib,re,sys
try:
 lines=pathlib.Path(sys.argv[1]).read_text().splitlines()
 signers={m[1].lower() for line in lines if (m:=re.fullmatch(r'Signer #[0-9]+ certificate SHA-256 digest: ([0-9a-fA-F]{64})',line.strip()))}
 pins={line.strip() for line in pathlib.Path(sys.argv[2]).read_text().splitlines() if line.strip() and not line.lstrip().startswith('#')}
 if not signers or any(not re.fullmatch(r'[0-9a-f]{64}',pin) for pin in pins): raise ValueError()
except (OSError,ValueError,UnicodeError):
 raise SystemExit('invalid APK signer output or pin file')
if signers!=pins: raise SystemExit(f'APK signer mismatch: APK {sorted(signers)}; pin {sorted(pins)}')
pathlib.Path(sys.argv[3]).write_text(json.dumps(sorted(signers)))
PY
cp "$apk" "$out/porchlight-$version.apk"
app=$root/client/desktopApp/build/compose/binaries/main/app
[[ -x $app/porchlight/bin/porchlight && -d $app/porchlight/lib/runtime ]] || { printf 'bundled desktop runtime missing\n' >&2; exit 1; }
# Dereference app-image links when packaging: installer forbids archive links/traversal.
tar --hard-dereference -chzf "$out/porchlight-desktop-$version-linux-x86_64.tar.gz" -C "$app" porchlight
cp deploy/compose.yml "$out/compose.yml"
cp deploy/compose.funnel.yml "$out/compose.funnel.yml"
python3 - deploy/env.example "$out/env.example" "$version" <<'PY'
import pathlib,re,sys
text=pathlib.Path(sys.argv[1]).read_text();text=re.sub(r'^PORCHLIGHT_VERSION=.*$', 'PORCHLIGHT_VERSION='+sys.argv[3],text,flags=re.M)
pathlib.Path(sys.argv[2]).write_text(text)
PY
python3 -I - "$out" <<'PY'
import pathlib,shutil,sys
out=pathlib.Path(sys.argv[1])
for name in ('lib','postgres'):
 shutil.copytree(pathlib.Path('deploy')/name,out/name,ignore=shutil.ignore_patterns('__pycache__','*.pyc'))
PY
cp design/icon/porchlight-icon.svg "$out/porchlight.svg"
python3 scripts/build-installer.py "$root" "$out/install.sh" "$public"
chmod 755 "$out/install.sh" "$out/porch-$version-x86_64-linux-musl"
python3 - "$root/server/db/migrations" "$out/migrations.json" <<'PY'
import hashlib,json,pathlib,sys
files={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in pathlib.Path(sys.argv[1]).glob('*.sql')}
pathlib.Path(sys.argv[2]).write_text(json.dumps(files,indent=2,sort_keys=True)+'\n')
PY
python3 -I - "$root" "$out" "$version" "$code" "$work/signers.json" <<'PY'
import json,os,pathlib,re,subprocess,sys
root,out=map(pathlib.Path,sys.argv[1:3]);version=sys.argv[3]
commit=None;dirty=True
try:
 result=subprocess.run(['git','-C',str(root),'rev-parse','HEAD'],check=True,capture_output=True,text=True)
 candidate=result.stdout.strip()
 if not re.fullmatch(r'[0-9a-f]{40}',candidate): raise ValueError()
 status=subprocess.run(['git','-C',str(root),'status','--porcelain'],check=True,capture_output=True,text=True)
 commit=candidate;dirty=bool(status.stdout)
except (OSError,subprocess.SubprocessError,ValueError): pass
archive=f'porchlight-desktop-{version}-linux-x86_64.tar.gz'
files=['install.sh','porchlight.svg',archive,'release.json']+[p.relative_to(out).as_posix() for p in (out/'lib').rglob('*') if p.is_file()]
release={'format_version':1,'version':version,'commit':commit,'dirty':dirty,
 'source':{'github':os.environ.get('PORCHLIGHT_RELEASE_REPO') or None},
 'android':{'apk':f'porchlight-{version}.apk','package':'app.porchlight','version_code':int(sys.argv[4]),
            'signer_sha256':json.loads(pathlib.Path(sys.argv[5]).read_text())},
 'desktop':{'archive':archive,'files':sorted(files)}}
(out/'release.json').write_text(json.dumps(release,indent=1,sort_keys=True)+'\n')
PY
python3 deploy/lib/operations.py manifest "$out" "$version"
minisign -Sm "$out/manifest.json" -s "$keys/minisign.key" -x "$out/manifest.json.minisig" >/dev/null 2>&1
verify_release "$out" "$public"
printf 'PASS verified signed release dist/%s\n' "$version"
