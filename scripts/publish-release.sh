#!/usr/bin/env bash
# Owner-only GitHub publication of an already signed dist; CI uses mocks, never real uploads.
# Loads the literal machine profile like deploy-server.sh. Dry-run performs every read-only check.
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$root"
# shellcheck source=scripts/local-env.sh
. "$root/scripts/local-env.sh"
porchlight_local_env "$root"
# shellcheck source=deploy/lib/operations.sh
. "$root/deploy/lib/operations.sh"
export PATH="$root/scripts/bin:$PATH" PYTHONDONTWRITEBYTECODE=1

die() { printf '%s\n' "$2" >&2; exit "$1"; }
[[ $# == 1 ]] || die 2 'usage: scripts/publish-release.sh <MAJOR.MINOR.PATCH>'
version=$1
code=$(version_code "$version") || die 2 'version must be MAJOR.MINOR.PATCH (minor/patch 0-99)'
[[ ${PORCHLIGHT_PUBLISH_DRY_RUN:-0} == 0 || ${PORCHLIGHT_PUBLISH_DRY_RUN:-0} == 1 ]] || die 2 'PORCHLIGHT_PUBLISH_DRY_RUN must be 0 or 1'
for tool in python3 minisign git gh tar; do command -v "$tool" >/dev/null || die 2 "required tool missing: $tool"; done
out=$root/dist/$version
[[ -d $out && ! -L $out ]] || die 2 'release directory missing or unsafe'
for name in manifest.json manifest.json.minisig release.json "porchlight-$version.apk" install.sh; do
  [[ -f $out/$name && ! -L $out/$name ]] || die 2 'required release file missing or unsafe'
done
public=$(python3 -I - "$root/deploy/release-keys/minisign.pub" <<'PY'
import base64,pathlib,sys
try:
 public=pathlib.Path(sys.argv[1]).read_text().splitlines()[-1]
 if len(base64.b64decode(public,validate=True))!=42: raise ValueError()
 print(public)
except (OSError,ValueError,IndexError,UnicodeError): sys.exit(2)
PY
) || die 2 'invalid committed minisign public key'
verify_release "$out" "$public" || die 1 'release verification failed'
apksigner=$(command -v apksigner || true)
if [[ -z $apksigner ]]; then
  apksigner=$(python3 -I - "${PORCHLIGHT_ANDROID_HOME:-${ANDROID_HOME:-}}" <<'PY'
import pathlib,sys
try:
 if not sys.argv[1]: raise ValueError()
 tools=pathlib.Path(sys.argv[1])/'build-tools'
 paths=[p/'apksigner' for p in tools.iterdir() if (p/'apksigner').is_file()]
 print(sorted(paths,key=lambda p:tuple(int(x) for x in p.parent.name.split('.') if x.isdigit()))[-1])
except (OSError,ValueError,IndexError): sys.exit(2)
PY
  ) || die 2 'apksigner needed on PATH or in the configured Android SDK'
fi
publish=$root/var/publish/$version
mkdir -p "$publish"
work=$(mktemp -d "$publish/work.XXXXXX")
trap 'rm -rf "$work"' EXIT
"$apksigner" verify --print-certs "$out/porchlight-$version.apk" > "$work/apk-certs" 2>/dev/null || die 1 'APK signature verification failed'
python3 -I - "$out" "$version" "$code" "$work/apk-certs" "$root/deploy/release-keys/android-signer.sha256" <<'PY'
import json,pathlib,re,sys
out=pathlib.Path(sys.argv[1]);version=sys.argv[2]
try:
 release=json.loads((out/'release.json').read_text()); manifest=json.loads((out/'manifest.json').read_text())
 android=release['android']
 if (release['format_version']!=1 or release['version']!=version or manifest['version']!=version
     or android['apk']!=f'porchlight-{version}.apk' or android['package']!='app.porchlight'
     or type(android['version_code']) is not int or android['version_code']!=int(sys.argv[3])): raise ValueError()
 recorded=android['signer_sha256']
 if not isinstance(recorded,list) or not recorded or any(not isinstance(x,str) or not re.fullmatch(r'[0-9a-f]{64}',x) for x in recorded): raise ValueError()
except (OSError,ValueError,KeyError,TypeError):
 print('invalid release.json metadata',file=sys.stderr);sys.exit(2)
try:
 lines=pathlib.Path(sys.argv[4]).read_text().splitlines()
 signers={m[1].lower() for line in lines if (m:=re.fullmatch(r'Signer #[0-9]+ certificate SHA-256 digest: ([0-9a-fA-F]{64})',line.strip()))}
 pins={line.strip() for line in pathlib.Path(sys.argv[5]).read_text().splitlines() if line.strip() and not line.lstrip().startswith('#')}
 if not signers or not pins or any(not re.fullmatch(r'[0-9a-f]{64}',pin) for pin in pins): raise ValueError()
except (OSError,ValueError,UnicodeError):
 raise SystemExit('invalid APK signer output or pin file')
if signers!=pins or signers!=set(recorded):
 raise SystemExit(f'APK signer mismatch: APK {sorted(signers)}; pin {sorted(pins)}; release.json {sorted(set(recorded))}')
PY

status=$(git -C "$root" status --porcelain 2>/dev/null) || die 2 'cannot read git working tree'
[[ -z $status ]] || die 2 'git working tree must be clean'
head=$(git -C "$root" rev-parse HEAD 2>/dev/null) || die 2 'cannot read git HEAD'
[[ $head =~ ^[0-9a-f]{40}$ ]] || die 2 'invalid git HEAD'
head_version=$(git -C "$root" show HEAD:VERSION 2>/dev/null) || die 2 'cannot read VERSION at HEAD'
[[ $head_version == "$version" ]] || die 2 'VERSION at HEAD differs from requested version'
python3 -I - "$out/release.json" "$head" <<'PY'
import json,pathlib,sys
release=json.loads(pathlib.Path(sys.argv[1]).read_text())
if release.get('commit')!=sys.argv[2]:
 print('release.json commit differs from HEAD',file=sys.stderr);sys.exit(2)
if release.get('dirty') is not False:
 print('release.json records a dirty build',file=sys.stderr);sys.exit(2)
PY
remote=$(git -C "$root" ls-remote origin refs/heads/main 2>/dev/null) || die 2 'cannot read remote main; push main first'
[[ $remote =~ ^([0-9a-f]{40})[[:space:]]refs/heads/main$ ]] || die 2 'remote main missing; push main first'
remote_sha=${BASH_REMATCH[1]}
git -C "$root" merge-base --is-ancestor HEAD "$remote_sha" >/dev/null 2>&1 || die 2 'HEAD is not on remote main; push main first'
repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner 2>/dev/null) || die 2 'cannot read GitHub repository; check gh auth login'
python3 -I - "$out/release.json" "$repo" <<'PY'
import json,os,pathlib,re,sys
release=json.loads(pathlib.Path(sys.argv[1]).read_text())
source=release.get('source')
source=source.get('github') if isinstance(source,dict) else None
profile=os.environ.get('PORCHLIGHT_RELEASE_REPO','')
if not isinstance(source,str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]{0,38}/[A-Za-z0-9._-]{1,100}',source):
 print('release.json needs a valid source.github',file=sys.stderr);sys.exit(2)
if source!=sys.argv[2] or source!=profile:
 print('release.json source.github, PORCHLIGHT_RELEASE_REPO and GitHub repository must match',file=sys.stderr);sys.exit(2)
PY
if gh release view "v$version" >/dev/null 2>&1; then die 2 'GitHub release already exists'; fi
tags=$(gh release list --limit 1000 --json tagName --jq '.[].tagName' 2>/dev/null) || die 2 'cannot list GitHub releases'
while IFS= read -r tag; do
  [[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || continue
  previous=$(version_code "${tag#v}") || die 2 'invalid published version tag'
  ((code > previous)) || die 2 'version must be newer than every published release'
done <<< "$tags"

archive=$publish/porchlight-$version-dist.tar
notes=$publish/notes.md
python3 -I - "$out" "$work/members" "$notes" "$root/deploy/release-keys/minisign.pub" <<'PY'
import base64,hashlib,json,pathlib,sys
try:
 out=pathlib.Path(sys.argv[1]);manifest=json.loads((out/'manifest.json').read_text());release=json.loads((out/'release.json').read_text())
 names=sorted(set(manifest['files'])|{'manifest.json','manifest.json.minisig'})
 if any(not (out/name).is_file() or (out/name).is_symlink() or '__pycache__' in pathlib.PurePosixPath(name).parts or name.endswith('.pyc') for name in names): raise ValueError()
 pathlib.Path(sys.argv[2]).write_bytes(b''.join(name.encode()+b'\0' for name in names))
 public=pathlib.Path(sys.argv[4]).read_text().splitlines()[-1]
 fingerprint=hashlib.sha256(base64.b64decode(public,validate=True)).hexdigest()
 apk=release['android']['apk'];sha=manifest['files'][apk]['sha256']
 lines=[f"# Porchlight {release['version']}",'',f"Commit: `{release['commit']}`",f'APK SHA-256: `{sha}`']
 lines += [f'APK signer SHA-256: `{signer}`' for signer in sorted(set(release['android']['signer_sha256']))]
 lines += [f'Minisign public key SHA-256: `{fingerprint}`','','Verify the signature and hashes as described in `docs/operations/install.md`.','']
 pathlib.Path(sys.argv[3]).write_text('\n'.join(lines))
except (OSError,ValueError,KeyError,TypeError,IndexError):
 print('cannot prepare release archive or notes',file=sys.stderr);sys.exit(2)
PY
# Explicit NUL-separated manifest paths: no ./ prefix, recursion, links or tar option injection.
tar --format=gnu --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner --hard-dereference \
  --no-recursion -cf "$archive" -C "$out" --null --verbatim-files-from -T "$work/members" || die 2 'cannot build release archive'
assets=("$out/manifest.json" "$out/manifest.json.minisig" "$archive" "$out/porchlight-$version.apk" "$out/install.sh")
create=(gh release create "v$version" --target "$head" --title "Porchlight $version" --notes-file "$notes" "${assets[@]}")
if [[ ${PORCHLIGHT_PUBLISH_DRY_RUN:-0} == 1 ]]; then
  printf 'DRY-RUN'; printf ' %q' "${create[@]}"; printf '\n'
  exit 0
fi
"${create[@]}" >/dev/null || die 2 'GitHub release creation failed (gh output above)'
mkdir -p "$publish/check"
check=$(mktemp -d "$publish/check/download.XXXXXX")
gh release download "v$version" --dir "$check" >/dev/null || die 1 'published release download failed (gh output above)'
for name in manifest.json manifest.json.minisig "porchlight-$version.apk" install.sh; do
  cmp -s "$out/$name" "$check/$name" || die 1 'published asset differs from local dist'
done
ops_python extract "$check/porchlight-$version-dist.tar" "$check/extracted" || die 1 'published archive extraction failed'
verify_release "$check/extracted" "$public" || die 1 'published archive verification failed'
# The tar must carry the same signed release, not another valid signed version.
cmp -s "$out/manifest.json" "$check/extracted/manifest.json" || die 1 'published archive manifest differs from local dist'
cmp -s "$out/manifest.json.minisig" "$check/extracted/manifest.json.minisig" || die 1 'published archive signature differs from local dist'
printf 'PASS published v%s\n' "$version"
