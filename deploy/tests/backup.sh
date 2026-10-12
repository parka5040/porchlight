#!/usr/bin/env bash
# Offline pipeline test: fake dump/crypto/SQL, never generates or consumes actual private keys.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$root/var/tests"
testdir=$(mktemp -d "$root/var/tests/backup.XXXXXX")
trap 'rm -rf "$testdir"' EXIT
export PYTHONDONTWRITEBYTECODE=1
export O1_FIXTURE="$testdir" PORCHLIGHT_DATA_DIR="$testdir/data" PORCHLIGHT_BACKUP_DIR="$testdir/backups" PORCHLIGHT_ENV_FILE="$testdir/stack.env"
export PORCHLIGHT_BACKUP_PASSWORD=not-a-real-password PORCHLIGHT_VERSION=0.1.0
export TMPDIR="$testdir/work" PORCHLIGHT_DUMP_TMPDIR="$testdir/disk"
# shellcheck disable=SC2174 # Both parents are the existing private mktemp testdir; only leaves are new.
mkdir -m 700 -p "$TMPDIR" "$PORCHLIGHT_DUMP_TMPDIR"
mkdir -p "$testdir/bin" "$testdir/data/pki" "$testdir/data/secrets" "$testdir/backups"
export PATH="$testdir/bin:$PATH"
python3 -I - "$testdir" <<'PY_BLOBS'
import hashlib,json,pathlib,sys
p=pathlib.Path(sys.argv[1]);rows=[]
for value in [b'BLOB-BYTES-never-stage-main',b'BLOB-BYTES-never-stage-thumb']:
 sha=hashlib.sha256(value).hexdigest();file=p/'data/blobs'/sha[:2]/sha[2:4]/sha
 file.parent.mkdir(parents=True,exist_ok=True);file.write_bytes(value)
 rows.append({'household_id':'h','sha256':sha,'size':len(value)})
(p/'blob-rows.json').write_text(json.dumps(rows))
(p/'blob-refs.json').write_text(json.dumps([{'household_id':'h','blob_sha256':rows[0]['sha256'],'thumbnail_sha256':rows[1]['sha256']}]))
PY_BLOBS
# Deliberately non-X509 public fixture. Fake openssl below is the only reader of fixture keys.
printf '%s\n' '-----BEGIN CERTIFICATE-----' 'Zml4dHVyZQ==' '-----END CERTIFICATE-----' > "$testdir/data/pki/ca.crt"
printf fixture > "$testdir/data/pki/ca.key"
printf fixture > "$testdir/data/pki/server.key"
cp "$testdir/data/pki/ca.crt" "$testdir/data/pki/server.crt"
printf fixture > "$testdir/data/secrets/idempotency.key"
printf 'age1-test-recipient\n' > "$testdir/data/backup-recipient.txt"
printf 'NOT_REAL_SECRET=fixture\n' > "$testdir/stack.env"
cat > "$testdir/bin/psql" <<'PY'
#!/usr/bin/env python3
import hashlib,json,os,pathlib,sys
for line in sys.stdin:
 state=pathlib.Path(os.environ['O1_FIXTURE'])/'row-status'
 if "WHERE status='running'" in line and state.exists() and state.read_text()=='running':
  state.write_text('failed');state.with_name('recovered-status').write_text('failed')
 elif 'RETURNING id' in line: state.write_text('running')
 elif "status='success'" in line: state.write_text('success')
 elif "status='failed'" in line and ("AND status='running'" not in line or state.read_text()=='running'): state.write_text('failed')
 with open(pathlib.Path(os.environ['O1_FIXTURE'])/'sql.log','a') as f: f.write(line)
 if 'RETURNING id' in line: print('00000000-0000-0000-0000-000000000001',flush=True)
 elif 'pl_take_blob_lease' in line: print('00000000-0000-0000-0000-000000000002',flush=True)
 elif 'pl_renew_blob_lease' in line: print('t',flush=True)
 elif 'pg_export_snapshot' in line: print('00000001-00000001-1',flush=True)
 elif 'migration_version' in line: print(json.dumps({'migration_version':50,'household_zone':'UTC','ca_fingerprint':hashlib.sha256(b'fixture').hexdigest()}),flush=True)
 elif 'pg_tables' in line: print('["backup_runs","households"]',flush=True)
 elif 'count(*)' in line: print('1',flush=True)
 elif 'to_regclass' in line: print('t',flush=True)
 elif 'json_agg(to_jsonb(b))' in line: print((pathlib.Path(os.environ['O1_FIXTURE'])/'blob-rows.json').read_text(),flush=True)
 elif 'json_agg(to_jsonb(a))' in line: print((pathlib.Path(os.environ['O1_FIXTURE'])/'blob-refs.json').read_text(),flush=True)
 elif 'COALESCE((SELECT' in line:
  if os.environ.get('O1_IDLE'):
   import datetime
   print(json.dumps({'finished_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'next_due_at':'2099-01-01T00:00:00Z'}),flush=True)
   pathlib.Path(os.environ['O1_FIXTURE'],'idle-ready').touch()
  else: print('{}',flush=True)
 elif 'SELECT 1;' in line: print('1',flush=True)
PY
cat > "$testdir/bin/pg_dump" <<'PY'
#!/usr/bin/env python3
import pathlib,sys,os,time
assert any(x.startswith('--snapshot=') for x in sys.argv)
destination=pathlib.Path(sys.argv[sys.argv.index('-f')+1])
assert pathlib.Path(os.environ['PORCHLIGHT_DUMP_TMPDIR']) in destination.parents
assert pathlib.Path(os.environ['TMPDIR']) not in destination.parents
assert destination.parent.stat().st_mode & 0o777 == 0o700
destination.write_bytes(b'fake custom format dump')
if os.environ.get('O1_BLOCK_DUMP'):
 pathlib.Path(os.environ['O1_FIXTURE'],'dump-ready').touch()
 time.sleep(120)
PY
cat > "$testdir/bin/age" <<'PY'
#!/usr/bin/env python3
import os,pathlib,shutil,sys
# Observe both staging trees while they exist, at encryption time. No blob bytes may be copied there.
for variable in ('TMPDIR','PORCHLIGHT_DUMP_TMPDIR'):
 for path in pathlib.Path(os.environ[variable]).rglob('*'):
  if path.is_file(): assert b'BLOB-BYTES' not in path.read_bytes(), 'attachment bytes were staged'
  assert not (path.name in ('file','thumb') and path.is_file()), 'attachment file was staged'
pathlib.Path(os.environ['O1_FIXTURE'],'no-staged-blobs').touch()
if os.environ.get('O1_FAIL_AGE'): sys.exit(1)
shutil.copyfileobj(sys.stdin.buffer,sys.stdout.buffer)
PY
cat > "$testdir/bin/openssl" <<'SH'
#!/usr/bin/env bash
printf 'FAKE-PUBLIC-KEY\n'
SH
chmod 755 "$testdir/bin/"*
bash "$root/deploy/backup/backup.sh" now
python3 - "$testdir" "$root" <<'PY'
import importlib.util,json,pathlib,sys
spec=importlib.util.spec_from_file_location('ops',sys.argv[2]+'/deploy/lib/operations.py');ops=importlib.util.module_from_spec(spec);spec.loader.exec_module(ops)
p=pathlib.Path(sys.argv[1]);bundles=list((p/'backups').glob('*.tar.age'));assert len(bundles)==1
ops.extract(bundles[0],p/'bundle');m=ops.verify(p/'bundle')
assert m['migration_version']==50 and m['table_row_counts']=={'backup_runs':1,'households':1}
assert (p/'bundle/.env').read_bytes()==(p/'stack.env').read_bytes()
assert (p/'no-staged-blobs').exists()
for name in m['referenced_blobs']:
 assert (p/'bundle/blobs'/name).read_bytes()==(p/'data/blobs'/name).read_bytes()
assert not list((p/'disk/porchlight-backup').glob('work-*'))
assert {'db.dump','pki/ca.key','secrets/idempotency.key','backup-recipient.txt','.env'} <= m['files'].keys()
assert not list((p/'backups').glob('.work-*'))
assert "status='success'" in (p/'sql.log').read_text()
spec=importlib.util.spec_from_file_location('backup',sys.argv[2]+'/deploy/backup/backup.py');backup=importlib.util.module_from_spec(spec);spec.loader.exec_module(backup)
class Snapshot:
 def __init__(self,values): self.values=iter(values)
 def query(self,_): return next(self.values)
# Final projection copies only snapshot references under the committed lease.
lease='00000000-0000-0000-0000-000000000002'
rows=[{'household_id':'h','sha256':format(i,'064x'),'size':1} for i in range(5000)]
refs=[{'blob_sha256':format(i,'064x'),'household_id':'h'} for i in range(5000)]
assert len(backup.referenced_blobs(Snapshot([json.dumps(rows),json.dumps(refs)]),lease))==5000
for values in [['[]','[{"household_id":"h","blob_sha256":"missing"}]'],[json.dumps(rows),'[{"blob_sha256":"'+format(1,'064x')+'","household_id":"other"}]'],['[{"household_id":"h","sha256":"../escape","size":1}]','[]']]:
 try: backup.referenced_blobs(Snapshot(values),lease)
 except ValueError: pass
 else: raise AssertionError('unleased/unsafe blob accepted')
log=(p/'sql.log').read_text()
assert log.index('pl_take_blob_lease') < log.index('pg_export_snapshot')
assert log.index("status='success'") < log.index('DELETE FROM porchlight.blob_leases')
(p/'linked').symlink_to(p/'data',target_is_directory=True)
try: backup.open_private_file(p/'linked/blobs/aa/aa/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')
except ValueError: pass
else: raise AssertionError('stream followed parent symlink')
try: backup.copy_private(p/'linked/backup-recipient.txt',p/'copied')
except ValueError: pass
else: raise AssertionError('parent symlink accepted')
PY
if O1_FAIL_AGE=1 bash "$root/deploy/backup/backup.sh" now >/dev/null 2>&1; then exit 1; fi
python3 - "$testdir" <<'PY'
import pathlib,sys
p=pathlib.Path(sys.argv[1]);assert len(list((p/'backups').glob('*.tar.age')))==1
assert not list((p/'backups').glob('.work-*'))
assert "status='failed'" in (p/'sql.log').read_text()
PY
python3 - "$testdir" "$root" <<'PY_MORE'
import importlib.util,pathlib,sys,os,subprocess,time,datetime,contextlib,io
p=pathlib.Path(sys.argv[1]);root=pathlib.Path(sys.argv[2])
spec=importlib.util.spec_from_file_location('backup',root/'deploy/backup/backup.py');b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
# Production run pruning, not just the selector: failure must preserve all old encrypted bundles.
old=[]
for day in range(100):
 name='porchlight-'+(datetime.datetime(2026,1,1)+datetime.timedelta(days=day)).strftime('%Y%m%dT%H%M%S%fZ')+'.tar.age'
 path=p/'backups'/name;path.write_bytes(b'encrypted fixture');old.append(path)
(p/'backups/unknown').write_text('keep')
os.environ['O1_FAIL_AGE']='1'
try: b.run()
except Exception: pass
else: raise AssertionError('expected encryption failure')
assert all(x.exists() for x in old)
del os.environ['O1_FAIL_AGE'];b.run()
assert sum(x.exists() for x in old) < len(old)
assert (p/'backups/unknown').exists()
# Actual SIGTERM mid-dump: process group stopped, work removed, failed status, no plaintext in publication.
env=os.environ.copy();env['O1_BLOCK_DUMP']='1'
child=subprocess.Popen(['bash',str(root/'deploy/backup/backup.sh'),'daemon'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
try:
 deadline=time.monotonic()+10
 while not (p/'dump-ready').exists():
  assert child.poll() is None
  assert time.monotonic()<deadline
  time.sleep(.02)
 subprocess.run(['bash',str(root/'deploy/backup/backup.sh'),'health'],check=True,stdout=subprocess.DEVNULL)
 assert list((p/'work/porchlight-backup').glob('work-*'))
 assert not list((p/'backups').glob('.work-*'))
 # A hung main operation must go stale: no independent thread or health query may pulse it.
 time.sleep(1.1) # Let the initial dump-file growth be observed before simulating a stall.
 os.utime(p/'backups/.heartbeat',(0,0));time.sleep(1.1)
 assert subprocess.run(['bash',str(root/'deploy/backup/backup.sh'),'health'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode!=0
 child.terminate();assert child.wait(timeout=8)!=0
finally:
 if child.poll() is None: child.kill();child.wait()
assert not list((p/'work/porchlight-backup').glob('work-*'))
assert not list((p/'disk/porchlight-backup').glob('work-*'))
assert not list((p/'backups').glob('.partial-*'))
assert "status='failed'" in (p/'sql.log').read_text().splitlines()[-1]
assert (p/'row-status').read_text()=='failed'
# Simulate SIGKILL leftovers; recovery holds lock and resets stale running rows before new INSERT.
stale=p/'work/porchlight-backup/work-stale';stale.mkdir();(stale/'db.dump').write_text('plaintext')
disk_stale=p/'disk/porchlight-backup/work-stale';disk_stale.mkdir();(disk_stale/'db.dump').write_text('plaintext')
legacy=p/'backups/.work-stale';legacy.mkdir();(legacy/'ca.key').write_text('fixture')
(p/'backups/.partial-stale').write_bytes(b'partial encrypted fixture')
(p/'row-status').write_text('running')
b.run();assert (p/'recovered-status').read_text()=='failed' and (p/'row-status').read_text()=='success'
assert not stale.exists() and not disk_stale.exists() and not legacy.exists() and not (p/'backups/.partial-stale').exists()
log=(p/'sql.log').read_text();assert "WHERE status='running'" in log
# Failure during retention after committed success may not change the published row.
retained=b.ops.retained
b.ops.retained=lambda _: (_ for _ in ()).throw(RuntimeError('pruning fixture'))
try: b.run()
except RuntimeError: pass
else: raise AssertionError('expected pruning error')
assert (p/'row-status').read_text()=='success'
b.ops.retained=retained
# Even a lost success-UPDATE response must not downgrade a committed success.
sql=b.sql
def lost_response(query):
 result=sql(query)
 if query.startswith('UPDATE') and "status='success'" in query: raise RuntimeError('lost SQL response')
 return result
b.sql=lost_response
try: b.run()
except RuntimeError: pass
else: raise AssertionError('expected lost response')
assert (p/'row-status').read_text()=='success'
b.sql=sql
# Idle daemon SIGTERM is a clean zero exit with no FAIL noise.
env=os.environ.copy();env['O1_IDLE']='1'
child=subprocess.Popen(['bash',str(root/'deploy/backup/backup.sh'),'daemon'],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
try:
 deadline=time.monotonic()+10
 while not (p/'idle-ready').exists():
  assert child.poll() is None and time.monotonic()<deadline;time.sleep(.02)
 child.terminate();out,err=child.communicate(timeout=8)
 assert child.returncode==0 and b'FAIL' not in err, (child.returncode,err)
finally:
 if child.poll() is None: child.kill();child.wait()
# Deterministic daemon/health/backoff loop; long catch-up is alive before running and during retries.
class EndLoop(BaseException): pass
sleeps=[]
b.sql=lambda q: '{}'
b.recover_locked=lambda _: None
def fail():
 assert (p/'backups/.heartbeat').exists()
 raise RuntimeError('fixture')
b.run=fail
def sleep(delay):
 sleeps.append(delay)
 if len(sleeps)==60: raise EndLoop()
b.time.sleep=sleep
with contextlib.redirect_stderr(io.StringIO()) as errors:
 try: b.daemon()
 except EndLoop: pass
assert 'FAIL backup job' in errors.getvalue()
assert sleeps==[60]*60
b.run=lambda: None;sleeps.clear()
def sleep(delay): sleeps.append(delay);raise EndLoop()
b.time.sleep=sleep
try: b.daemon()
except EndLoop: pass
assert sleeps==[60]
b.sql=lambda _: '1';b.health()
os.utime(p/'backups/.heartbeat',(0,0))
try: b.health()
except AssertionError: pass
else: raise AssertionError('stale heartbeat accepted')
print('PASS no blob staging, disk dump cleanup/recovery, progress health/hung detection, published success survives pruning failure, clean idle stop/backoff')
PY_MORE
printf 'PASS backup snapshot, bundle members, atomic publication, failure preserves previous bundle\n'
