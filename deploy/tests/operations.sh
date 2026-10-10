#!/usr/bin/env bash
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1
root=$(cd "$(dirname "$0")/../.." && pwd)
. "$root/deploy/lib/operations.sh"
# All fixtures (including fake .env/PKI) are runtime state under var, never real keys.
mkdir -p "$root/var/tests"
testdir=$(mktemp -d "$root/var/tests/o1.XXXXXX")
trap 'rm -rf "$testdir"' EXIT
export TMPDIR="$testdir"
[[ $(version_code 0.1.0) == 100 ]]
[[ $(version_code 12.34.56) == 123456 ]]
[[ $(retention_selection unrelated-file) == '' ]]
[[ $(retention_selection porchlight-20260101T030000000000Z.tar.age) == porchlight-20260101T030000000000Z.tar.age ]]
mkdir "$testdir/backups"
printf 'fake encrypted bundle' > "$testdir/backups/porchlight-20260101T030000000000Z.tar.age"
pinned=$(pin_rollback_bundle "$testdir/backups/porchlight-20260101T030000000000Z.tar.age")
rm "$testdir/backups/porchlight-20260101T030000000000Z.tar.age"
[[ -f $pinned && $(< "$pinned") == 'fake encrypted bundle' ]]
for bad in 01.2.3 1.100.0 1.0 1.2.3-rc1 999999.0.0; do
  if version_code "$bad" >/dev/null 2>&1; then exit 1; fi
done
[[ $(source_kind '/local/path with spaces') == local ]]
[[ $(source_kind 'owner@laptop:/release/path') == ssh ]]
for bad in '-option' 'https://example.test' 'owner@host:relative' 'host:/x'; do
  if source_kind "$bad" >/dev/null 2>&1; then exit 1; fi
done
mkdir "$testdir/release"
printf payload > "$testdir/release/artifact"
python3 "$root/deploy/lib/operations.py" manifest "$testdir/release" 0.1.0
verify_hashes "$testdir/release"
printf corrupt >> "$testdir/release/artifact"
if verify_hashes "$testdir/release" >/dev/null 2>&1; then exit 1; fi
python3 - "$testdir" "$root" <<'PY'
import importlib.util,json,pathlib,sys,datetime,tarfile,io
spec=importlib.util.spec_from_file_location('ops',sys.argv[2]+'/deploy/lib/operations.py'); ops=importlib.util.module_from_spec(spec);spec.loader.exec_module(ops)
p=pathlib.Path(sys.argv[1])/'release'
m=json.loads((p/'manifest.json').read_text());m['files']['../escape']=m['files'].pop('artifact');(p/'manifest.json').write_text(json.dumps(m))
try: ops.verify(p)
except (ValueError,FileNotFoundError): pass
else: raise AssertionError('traversal accepted')
# Operator-owned symlink ancestors are valid; release-owned symlink components are not.
ancestor=pathlib.Path(sys.argv[1])/'symlink-ancestor';ancestor.symlink_to(pathlib.Path(sys.argv[1]),target_is_directory=True)
good=pathlib.Path(sys.argv[1])/'good';good.mkdir();(good/'artifact').write_text('fixture');ops.manifest(good,'0.1.0')
ops.verify(ancestor/'good')
(good/'dir').symlink_to(good,target_is_directory=True)
try: ops.verify(good)
except ValueError: pass
else: raise AssertionError('release-owned symlink accepted')
# A desktop subset must retain the signed descriptor and every required desktop member.
subset=pathlib.Path(sys.argv[1])/'subset';subset.mkdir()
for name in ('install.sh','porchlight.svg','porchlight-desktop-0.1.6-linux-x86_64.tar.gz','server.tar'):
 (subset/name).write_text('fixture')
record={'format_version':1,'version':'0.1.6','commit':None,'dirty':True,
        'source':{'github':'example-owner/porchlight'},
        'android':{'apk':'porchlight-0.1.6.apk','package':'app.porchlight','version_code':106,'signer_sha256':['a'*64]},
        'desktop':{'archive':'porchlight-desktop-0.1.6-linux-x86_64.tar.gz','files':sorted(['porchlight-desktop-0.1.6-linux-x86_64.tar.gz','install.sh','porchlight.svg','release.json'])}}
(subset/'release.json').write_text(json.dumps(record,indent=1,sort_keys=True)+'\n')
ops.manifest(subset,'0.1.6');(subset/'server.tar').unlink()
ops.verify(subset,subset=True)
for name,content in [('extra','fixture'),('porchlight.svg','tampered')]:
 original=(subset/name).read_bytes() if (subset/name).exists() else None
 (subset/name).write_text(content)
 try: ops.verify(subset,subset=True)
 except ValueError: pass
 else: raise AssertionError('invalid subset accepted')
 if original is None: (subset/name).unlink()
 else: (subset/name).write_bytes(original)
for name in ('release.json','install.sh'):
 original=(subset/name).read_bytes();(subset/name).unlink()
 try: ops.verify(subset,subset=True)
 except ValueError: pass
 else: raise AssertionError('incomplete subset accepted')
 (subset/name).write_bytes(original)
(subset/'extra-link').symlink_to(subset/'install.sh')
try: ops.verify(subset,subset=True)
except ValueError: pass
else: raise AssertionError('subset symlink accepted')
print('PASS desktop subset inventory, required members, hashes and symlink refusal')
# Latest per day, latest per ISO week, union; not simply the latest 22 files.
names=['porchlight-'+(datetime.datetime(2026,1,1,tzinfo=datetime.timezone.utc)+datetime.timedelta(days=i)).strftime('%Y%m%dT%H%M%S%fZ')+'.tar.age' for i in range(100)]
keep=ops.retained(names)
assert len(keep)==19, len(keep)
assert set(names[-14:]) <= set(keep)
assert ops.retained(['unrelated',*names])==keep
assert ops.next_due('America/New_York','2026-03-07T12:00:00Z')=='2026-03-08T07:00:00Z'
assert ops.next_due('America/New_York','2026-11-01T08:00:00Z')=='2026-11-02T08:00:00Z'
assert ops.next_due('UTC','2026-10-05T02:59:59Z')=='2026-10-05T03:00:00Z'
assert ops.backup_due(None,None,'2026-10-05T00:00:00Z')
assert ops.backup_due('2026-10-03T00:00:00Z','2026-10-06T03:00:00Z','2026-10-05T00:00:00Z')
assert ops.backup_due('2026-10-05T00:00:00Z','2026-10-05T03:00:00Z','2026-10-05T03:00:00Z')
assert not ops.backup_due('2026-10-05T00:00:00Z','2026-10-06T03:00:00Z','2026-10-05T12:00:00Z')
# Archive extraction never follows symlinks or permits parent/absolute paths.
for name,kind in [('../escape',tarfile.REGTYPE),('/escape',tarfile.REGTYPE),('pki/link',tarfile.SYMTYPE)]:
 a=pathlib.Path(sys.argv[1])/'bad.tar'
 with tarfile.open(a,'w') as t:
  h=tarfile.TarInfo(name);h.type=kind;h.linkname='/etc';t.addfile(h)
 try: ops.extract(a,pathlib.Path(sys.argv[1])/'extract')
 except ValueError: pass
 else: raise AssertionError('unsafe archive accepted')
print('PASS manifest, retention, scheduling, safe extraction')
PY
printf 'PASS versionCode and source parsing\n'

# Missing bootstrap credential must fail before invoking psql, with a useful non-secret message.
if (export PORCHLIGHT_MIGRATOR_PASSWORD=fixture PORCHLIGHT_APP_PASSWORD=fixture PORCHLIGHT_AGENT_PASSWORD=fixture; unset PORCHLIGHT_BACKUP_PASSWORD; sh "$root/deploy/postgres/initdb/10-porchlight.sh") > "$testdir/bootstrap.log" 2>&1; then exit 1; fi
[[ $(< "$testdir/bootstrap.log") == *'PORCHLIGHT_BACKUP_PASSWORD is required'* ]]
# Existing dev env gets exactly one appended backup credential, without rotating any existing value.
mkdir -p "$testdir/dev/scripts" "$testdir/dev/var" "$testdir/dev/bin"
cp "$root/scripts/dev-env.sh" "$testdir/dev/scripts/"
cp "$root/VERSION" "$testdir/dev/VERSION"
printf 'POSTGRES_PASSWORD=fixture\nPORCHLIGHT_DEV_ADMIN_PASSWORD=fixture\n' > "$testdir/dev/var/dev.env"
cp "$testdir/dev/var/dev.env" "$testdir/env.before"
printf '#!/bin/sh\nprintf "fixture-random-value\\n"\n' > "$testdir/dev/bin/openssl"
chmod 755 "$testdir/dev/bin/openssl"
PATH="$testdir/dev/bin:$PATH" bash "$testdir/dev/scripts/dev-env.sh" >/dev/null
PATH="$testdir/dev/bin:$PATH" bash "$testdir/dev/scripts/dev-env.sh" >/dev/null
python3 - "$testdir" <<'PY_DEV'
from pathlib import Path
import sys
p=Path(sys.argv[1]);before=(p/'env.before').read_text();after=(p/'dev/var/dev.env').read_text()
assert after==before+'PORCHLIGHT_BACKUP_PASSWORD=fixture-random-value\nPORCHLIGHT_VERSION='+(p/'dev/VERSION').read_text().strip()+'\n'
assert (p/'dev/var/dev.env').stat().st_mode & 0o777 == 0o600
PY_DEV
printf 'PASS fail-fast bootstrap and existing dev-env append without rotation\n'
