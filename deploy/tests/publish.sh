#!/usr/bin/env bash
# Offline owner-publish contract: every external authority is a logged, environment-driven shim.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$root/var/tests"
fixture=$(mktemp -d "$root/var/tests/publish.XXXXXX")
trap 'rm -rf "$fixture"' EXIT
export PYTHONDONTWRITEBYTECODE=1
unset PORCHLIGHT_RELEASE_REPO PORCHLIGHT_PUBLISH_DRY_RUN
mkdir -p "$fixture/scripts/bin" "$fixture/deploy/lib" "$fixture/deploy/release-keys" "$fixture/dist/0.1.6/lib"
cp "$root/scripts/publish-release.sh" "$root/scripts/local-env.sh" "$fixture/scripts/"
cp "$root/deploy/lib/operations."{sh,py} "$fixture/deploy/lib/"
printf 'PORCHLIGHT_RELEASE_REPO=example-owner/porchlight\n' > "$fixture/.env"
printf '0.1.6\n' > "$fixture/VERSION"
export MOCK_ROOT=$fixture MOCK_LOG=$fixture/argv.jsonl
export PATH="$fixture/scripts/bin:$PATH"
python3 -I - "$fixture" <<'PY'
import base64,pathlib,sys
root=pathlib.Path(sys.argv[1]);keys=root/'deploy/release-keys'
(keys/'minisign.pub').write_text('untrusted comment: fixture\n'+base64.b64encode(bytes(42)).decode()+'\n')
(keys/'android-signer.sha256').write_text('# Fixture pin\n\n'+'a'*64+'\n')
out=root/'dist/0.1.6'
for name in ('install.sh','porchlight-0.1.6.apk','porchlight-desktop-0.1.6-linux-x86_64.tar.gz','porchlight.svg','lib/operations.py'):
 (out/name).write_text('fixture '+name+'\n')
PY
# These files are generated here, not downloaded; no real git/gh/signing tool can be reached.
for tool in git gh apksigner minisign; do
  cat > "$fixture/scripts/bin/$tool" <<'PY'
#!/usr/bin/env python3
import hashlib,io,json,os,pathlib,shutil,sys,tarfile
name=pathlib.Path(sys.argv[0]).name;args=sys.argv[1:];root=pathlib.Path(os.environ['MOCK_ROOT'])
with open(os.environ['MOCK_LOG'],'a') as log: log.write(json.dumps([name,*args])+'\n')
if os.environ.get('FAIL_TOOL')==name: sys.exit(1)
if name=='git':
 assert args[:2]==['-C',str(root)];args=args[2:]
 if args==['status','--porcelain']: print(os.environ.get('GIT_STATUS',''),end='')
 elif args==['rev-parse','HEAD']: print(os.environ.get('GIT_HEAD','a'*40))
 elif args==['show','HEAD:VERSION']: print(os.environ.get('GIT_VERSION','0.1.6'))
 elif args==['ls-remote','origin','refs/heads/main']:
  if os.environ.get('GIT_NO_MAIN')=='1': sys.exit(0)
  print('b'*40+'\trefs/heads/main')
 elif args==['merge-base','--is-ancestor','HEAD','b'*40]: sys.exit(int(os.environ.get('GIT_NOT_PUSHED','0')))
 else: sys.exit(99)
elif name=='minisign':
 assert args[0]=='-Vm'
 if os.environ.get('FAIL_RELEASE_SIGNATURE')=='1': sys.exit(1)
 manifest=pathlib.Path(args[1]);signature=pathlib.Path(args[args.index('-x')+1])
 sys.exit(0 if signature.read_text()==hashlib.sha256(manifest.read_bytes()).hexdigest() else 1)
elif name=='apksigner':
 assert args[:2]==['verify','--print-certs'] and pathlib.Path(args[2]).is_file()
 if os.environ.get('FAIL_APK_SIGNATURE')=='1': sys.exit(1)
 for i,digest in enumerate(os.environ.get('APK_SIGNERS','a'*64).split(','),1):
  print(f'Signer #{i} certificate SHA-256 digest: {digest}')
elif name=='gh':
 if args==['repo','view','--json','nameWithOwner','--jq','.nameWithOwner']:
  print(os.environ.get('GH_REPO','example-owner/porchlight'))
 elif args==['release','view','v0.1.6']: sys.exit(0 if os.environ.get('GH_EXISTS')=='1' else 1)
 elif args==['release','list','--limit','1000','--json','tagName','--jq','.[].tagName']:
  print(os.environ.get('GH_TAGS','nightly\nv0.1.5\nv0.1.4'))
 elif args[:2]==['release','create']:
  uploaded=root/'uploaded';shutil.rmtree(uploaded,ignore_errors=True);uploaded.mkdir()
  for asset in args[-5:]: shutil.copy2(asset,uploaded)
 elif args[:3]==['release','download','v0.1.6']:
  assert args[3]=='--dir';out=pathlib.Path(args[4]);assert not list(out.iterdir())
  for asset in (root/'uploaded').iterdir(): shutil.copy2(asset,out)
  bad=os.environ.get('ROUND_TRIP','')
  if bad=='download-failure': sys.exit(1)
  if bad in ('manifest.json','manifest.json.minisig','porchlight-0.1.6.apk','install.sh'):
   (out/bad).write_text('tampered')
  elif bad in ('tar-hash','tar-path'):
   archive=out/'porchlight-0.1.6-dist.tar'
   if bad=='tar-hash':
    with tarfile.open(archive,'a') as tar:
     info=tarfile.TarInfo('extra');info.size=1;tar.addfile(info,io.BytesIO(b'x'))
   else:
    with tarfile.open(archive,'w') as tar:
     info=tarfile.TarInfo('../escape');info.size=1;tar.addfile(info,io.BytesIO(b'x'))
 else: sys.exit(99)
else: sys.exit(99)
PY
  chmod 755 "$fixture/scripts/bin/$tool"
done
chmod 755 "$fixture/scripts/publish-release.sh"
command=(bash "$fixture/scripts/publish-release.sh" 0.1.6)
reset_metadata() {
  python3 -I - "$fixture/dist/0.1.6" <<'PY'
import json,pathlib,sys
out=pathlib.Path(sys.argv[1]);archive='porchlight-desktop-0.1.6-linux-x86_64.tar.gz'
release={'format_version':1,'version':'0.1.6','commit':'a'*40,'dirty':False,
 'source':{'github':'example-owner/porchlight'},
 'android':{'apk':'porchlight-0.1.6.apk','package':'app.porchlight','version_code':106,'signer_sha256':['a'*64]},
 'desktop':{'archive':archive,'files':sorted(['install.sh','lib/operations.py',archive,'porchlight.svg','release.json'])}}
(out/'release.json').write_text(json.dumps(release,indent=1,sort_keys=True)+'\n')
PY
  sign_fixture
}
sign_fixture() {
  python3 -I "$fixture/deploy/lib/operations.py" manifest "$fixture/dist/0.1.6" 0.1.6
  python3 -I - "$fixture/dist/0.1.6" <<'PY'
import hashlib,pathlib,sys
out=pathlib.Path(sys.argv[1]);(out/'manifest.json.minisig').write_text(hashlib.sha256((out/'manifest.json').read_bytes()).hexdigest())
PY
}
no_upload() {
  python3 -I - "$MOCK_LOG" <<'PY'
import json,pathlib,sys
calls=[json.loads(line) for line in pathlib.Path(sys.argv[1]).read_text().splitlines()]
assert not any(call[:3] in (['gh','release','create'],['gh','release','download']) for call in calls)
PY
}
refuse() {
  local code=$1 message=$2 status=0; shift 2
  : > "$MOCK_LOG"
  "$@" > "$fixture/result.log" 2>&1 || status=$?
  [[ $status == "$code" ]] || { printf 'unexpected refusal status: %s (wanted %s): %s\n' "$status" "$code" "$message" >&2; exit 1; }
  grep -qF "$message" "$fixture/result.log"
  if [[ $message != published* ]]; then no_upload; fi
}
reset_metadata
refuse 2 'usage:' bash "$fixture/scripts/publish-release.sh"
refuse 2 'version must be' bash "$fixture/scripts/publish-release.sh" 00.1.6
refuse 2 'version must be' bash "$fixture/scripts/publish-release.sh" 0.1.100
refuse 2 'release directory missing' bash "$fixture/scripts/publish-release.sh" 0.1.7
refuse 2 'PORCHLIGHT_PUBLISH_DRY_RUN must be' env PORCHLIGHT_PUBLISH_DRY_RUN=bad "${command[@]}"
refuse 1 'release verification failed' env FAIL_RELEASE_SIGNATURE=1 "${command[@]}"
printf 'tampered\n' >> "$fixture/dist/0.1.6/install.sh"
refuse 1 'release verification failed' "${command[@]}"
printf 'fixture install.sh\n' > "$fixture/dist/0.1.6/install.sh"
refuse 1 'APK signature verification failed' env FAIL_APK_SIGNATURE=1 "${command[@]}"
refuse 1 'APK signer mismatch' env APK_SIGNERS="$(printf '%064d' 2)" "${command[@]}"
refuse 1 'invalid APK signer output or pin file' env APK_SIGNERS= "${command[@]}"
refuse 2 'working tree must be clean' env GIT_STATUS=' M VERSION' "${command[@]}"
refuse 2 'VERSION at HEAD differs' env GIT_VERSION=0.1.5 "${command[@]}"
refuse 2 'commit differs from HEAD' env GIT_HEAD="$(printf '%040d' 2)" "${command[@]}"
refuse 2 'push main first' env GIT_NOT_PUSHED=1 "${command[@]}"
refuse 2 'push main first' env GIT_NO_MAIN=1 "${command[@]}"
refuse 2 'cannot read git working tree' env FAIL_TOOL=git "${command[@]}"
refuse 2 'cannot read GitHub repository' env FAIL_TOOL=gh "${command[@]}"
refuse 2 'GitHub repository must match' env GH_REPO=example-owner/other "${command[@]}"
refuse 2 'GitHub repository must match' env PORCHLIGHT_RELEASE_REPO=example-owner/other "${command[@]}"
refuse 2 'GitHub release already exists' env GH_EXISTS=1 "${command[@]}"
refuse 2 'newer than every published release' env GH_TAGS=v0.1.6 "${command[@]}"
refuse 2 'newer than every published release' env GH_TAGS=$'v0.1.5\nv1.0.0\nv0.1.4' "${command[@]}"
no_upload
for mutation in dirty commit source signer version; do
  python3 -I - "$fixture/dist/0.1.6/release.json" "$mutation" <<'PY'
import json,pathlib,sys
path=pathlib.Path(sys.argv[1]);release=json.loads(path.read_text());mutation=sys.argv[2]
if mutation=='dirty': release['dirty']=True
elif mutation=='commit': release['commit']=None
elif mutation=='source': release['source']['github']=None
elif mutation=='signer': release['android']['signer_sha256']=['b'*64]
elif mutation=='version': release['version']='0.1.5'
path.write_text(json.dumps(release))
PY
  sign_fixture
  case $mutation in
    dirty) refuse 2 'dirty build' "${command[@]}" ;;
    commit) refuse 2 'commit differs from HEAD' "${command[@]}" ;;
    source) refuse 2 'valid source.github' "${command[@]}" ;;
    signer) refuse 1 'APK signer mismatch' "${command[@]}" ;;
    version) refuse 2 'invalid release.json metadata' "${command[@]}" ;;
  esac
  no_upload
  reset_metadata
done
: > "$MOCK_LOG"
PORCHLIGHT_PUBLISH_DRY_RUN=1 "${command[@]}" > "$fixture/result.log"
no_upload
python3 -I - "$fixture" <<'PY'
import base64,hashlib,json,pathlib,subprocess,sys,tarfile
root=pathlib.Path(sys.argv[1]);out=root/'dist/0.1.6';publish=root/'var/publish/0.1.6'
argv=['gh','release','create','v0.1.6','--target','a'*40,'--title','Porchlight 0.1.6','--notes-file',str(publish/'notes.md'),
      str(out/'manifest.json'),str(out/'manifest.json.minisig'),str(publish/'porchlight-0.1.6-dist.tar'),str(out/'porchlight-0.1.6.apk'),str(out/'install.sh')]
expected=subprocess.check_output(['bash','-c',"printf 'DRY-RUN'; printf ' %q' \"$@\"; printf '\\n'",'fixture',*argv],text=True)
assert (root/'result.log').read_text()==expected
calls=[json.loads(line) for line in (root/'argv.jsonl').read_text().splitlines()]
assert any(call[0]=='git' and 'ls-remote' in call for call in calls)
assert ['gh','release','view','v0.1.6'] in calls
assert ['gh','release','list','--limit','1000','--json','tagName','--jq','.[].tagName'] in calls
with tarfile.open(publish/'porchlight-0.1.6-dist.tar') as tar:
 members=tar.getmembers();names=[m.name for m in members]
 assert names==sorted([*json.loads((out/'manifest.json').read_text())['files'],'manifest.json','manifest.json.minisig'])
 assert all(not m.name.startswith('./') and m.isfile() and m.mtime==0 and m.uid==0 and m.gid==0 for m in members)
notes=(publish/'notes.md').read_text();manifest=json.loads((out/'manifest.json').read_text())
assert 'Porchlight 0.1.6' in notes and 'a'*40 in notes and 'a'*64 in notes
assert manifest['files']['porchlight-0.1.6.apk']['sha256'] in notes
assert hashlib.sha256(bytes(42)).hexdigest() in notes
assert 'docs/operations/install.md' in notes and 'example-owner' not in notes
(root/'expected-create.json').write_text(json.dumps(argv))
PY
cp "$fixture/var/publish/0.1.6/porchlight-0.1.6-dist.tar" "$fixture/first.tar"
# Ignore duplicate digest lines and case in apksigner, and prove tar reproducibility.
PORCHLIGHT_PUBLISH_DRY_RUN=1 APK_SIGNERS="$(printf 'A%.0s' {1..64}),$(printf 'a%.0s' {1..64})" "${command[@]}" > /dev/null
cmp "$fixture/first.tar" "$fixture/var/publish/0.1.6/porchlight-0.1.6-dist.tar"
: > "$MOCK_LOG"
"${command[@]}" > "$fixture/result.log"
grep -qx 'PASS published v0.1.6' "$fixture/result.log"
python3 -I - "$fixture" <<'PY'
import json,pathlib,sys
root=pathlib.Path(sys.argv[1]);calls=[json.loads(line) for line in (root/'argv.jsonl').read_text().splitlines()]
assert json.loads((root/'expected-create.json').read_text()) in calls
assert sum(call[:3]==['gh','release','create'] for call in calls)==1
assert sum(call[:3]==['gh','release','download'] for call in calls)==1
publish=root/'var/publish/0.1.6'
assert not list((publish/'check').glob('download.*')), 'F1-07: successful round trip retains downloads/extraction'
assert (publish/'porchlight-0.1.6-dist.tar').is_file() and (root/'dist/0.1.6/manifest.json').is_file()
PY
refuse 1 'published release download failed' env ROUND_TRIP=download-failure "${command[@]}"
for asset in manifest.json manifest.json.minisig porchlight-0.1.6.apk install.sh; do
  refuse 1 'published asset differs' env ROUND_TRIP="$asset" "${command[@]}"
done
refuse 1 'published archive extraction failed' env ROUND_TRIP=tar-hash "${command[@]}"
refuse 1 'published archive extraction failed' env ROUND_TRIP=tar-path "${command[@]}"
python3 -I - "$fixture/var/publish/0.1.6" <<'PY'
import pathlib,sys
publish=pathlib.Path(sys.argv[1]);checks=list((publish/'check').glob('download.*'))
assert len(checks)==7, 'failed download/verification diagnostics must remain'
assert all((p/'porchlight-0.1.6-dist.tar').is_file() for p in checks)
assert not list(publish.glob('work.*'))
PY
printf 'PASS publish refusals, exact dry-run/create argv, deterministic tar, notes and verified round trip\n'
