#!/usr/bin/env bash
# Offline controller contract: fake transport/Docker; real TLS only on loopback fixture servers.
set -euo pipefail
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$root/var/tests"
fixture=$(mktemp -d "$root/var/tests/deploy-server.XXXXXX")
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/scripts" "$fixture/bin" "$fixture/var" "$fixture/dist/0.1.0"
# This deliberately fails first when the implementation has not been created yet.
cp "$root/scripts/deploy-server.sh" "$root/scripts/local-env.sh" "$fixture/scripts/"
export FIXTURE="$fixture" SHIM_LOG="$fixture/var/shims.jsonl"
export PORCHLIGHT_DEPLOY_HOST=offline-test-host PORCHLIGHT_DEPLOY_DATA_MOUNT=/srv/data PORCHLIGHT_DEPLOY_LAN=192.168.77.50/24
# Keep caller tool wrappers out: only fake transports plus system shell/Python tools.
export PATH="$fixture/bin:/usr/bin:/bin"
cat > "$fixture/bin/ssh" <<'PY'
#!/usr/bin/env -S python3 -I
import hashlib,json,os,pathlib,sys
args=sys.argv[1:];data=sys.stdin.buffer.read();cmd=args[-1]
with open(os.environ['SHIM_LOG'],'a') as f: f.write(json.dumps({'tool':'ssh','argv':args,'stdin_bytes':len(data)})+'\n')
assert 'offline-test-host' in args
if '# preflight' in cmd:
 for name in ['nixos','group','run-dir','root-dir','dist-dir','data-mount','minisign','age','rsync','python3','docker','docker-mount','compose-root','port-8443','port-8444']:
  print(('FAIL' if os.getenv('FAIL_CHECK')==name else 'PASS')+' '+name)
 sys.exit(1 if os.getenv('FAIL_CHECK') else 0)
elif '# stack-absent' in cmd: sys.exit(int(os.getenv('STACK_CHECK_STATUS','1' if os.getenv('STACK_EXISTS') else '0')))
elif '# release-hashes' in cmd:
 p=pathlib.Path(os.environ['FIXTURE'])/'dist/0.1.0'
 for name in ['install.sh','manifest.json']:
  print(('0'*64 if os.getenv('HASH_MISMATCH') else hashlib.sha256((p/name).read_bytes()).hexdigest())+'  '+name)
elif '# porchlight-gid' in cmd: print('987')
elif '# enroll-uri' in cmd:
 if os.getenv('MOCK_EXEC_ENROLL'):
  # Execute the generated remote script locally, against a fake release/CLI only.
  import shlex,subprocess
  script=shlex.split(cmd)[-1].replace('/srv/data/porchlight',str(pathlib.Path(os.environ['FIXTURE'])/'remote/stack'))
  sys.exit(subprocess.run(['bash','-c',script]).returncode)
 failure=os.getenv('ENROLL_FAIL')
 if failure=='problem':
  print(json.dumps({'type':'urn:porchlight:problem:not-found','title':'Not found','status':404,'detail':'Wrong user: requested member does not exist','code':'not-found','token':'FAILED-TOKEN-DISTINCT-MARKER','extra':'FAILED-EXTRA-DISTINCT-MARKER'}))
 elif failure=='malformed': print('FAILED-RAW-DISTINCT-MARKER')
 elif failure=='unsafe':
  print(json.dumps({'type':{},'title':['FAILED-NESTED-DISTINCT-MARKER'],'status':False,'detail':'safe\u0000\u001b\nline'+('x'*1000),'code':{'token':'FAILED-NESTED-DISTINCT-MARKER'}}))
 else:
  print(json.dumps({'uri':('porchlight://FAILED-REQUEST-DISTINCT-MARKER' if failure else 'porchlight://SECRET-ENROLLMENT-FIXTURE'),'detail':'FAILED-URI-DETAIL-DISTINCT-MARKER'}))
 sys.exit(1 if failure else 0)
elif any(marker in cmd for marker in ['# funnel-key', '# funnel-activate', '# funnel-status', '# funnel-ca']):
 if '# funnel-status' in cmd and os.getenv('STATUS_SSH_FAIL'): sys.exit(255)
 import shlex,subprocess
 script=shlex.split(cmd)[-1].replace('/srv/data/porchlight',str(pathlib.Path(os.environ['FIXTURE'])/'remote/stack'))
 if '# funnel-activate' in cmd and os.getenv('STREAM_WRITE_FAIL'):
  script='cat() { command cat "$@"; return 1; }\n'+script
 sys.exit(subprocess.run(['bash','-c',script],input=data).returncode)
elif '# status' in cmd: print('PASS status fixture')
elif '# backup' in cmd: print('PASS backup porchlight-20261007T030000000000Z.tar.age')
PY
cat > "$fixture/bin/rsync" <<'PY'
#!/usr/bin/env -S python3 -I
import json,os,sys
with open(os.environ['SHIM_LOG'],'a') as f: f.write(json.dumps({'tool':'rsync','argv':sys.argv[1:],'stdin_bytes':len(sys.stdin.buffer.read())})+'\n')
PY
cat > "$fixture/bin/age-keygen" <<'PY'
#!/usr/bin/env -S python3 -I
import json,os,pathlib,sys
# Match scripts/bin/_tool: dispatch depends on the invoked symlink name.
assert pathlib.Path(sys.argv[0]).name=='age-keygen'
assert sys.argv[1]=='-y'
if '/scripts/bin/' in sys.argv[0]:
 assert os.getcwd()==os.environ['FIXTURE']
 assert sys.argv[2]=='var/backup-recovery.key'
with open(os.environ['SHIM_LOG'],'a') as f: f.write(json.dumps({'tool':'age-keygen','argv':sys.argv[1:],'stdin_bytes':len(sys.stdin.buffer.read())})+'\n')
print('age1fixturepublicrecipient')
PY
chmod 755 "$fixture/bin/"* "$fixture/scripts/deploy-server.sh"
run() {
  local expected=$1 actual=0; shift
  : > "$SHIM_LOG"
  bash "$fixture/scripts/deploy-server.sh" "$@" < /dev/null > "$fixture/var/stdout" 2> "$fixture/var/stderr" || actual=$?
  [[ $actual == "$expected" ]] || { printf 'FAIL deploy-server expected exit %s, got %s\n' "$expected" "$actual" >&2; exit 1; }
  for log in "$SHIM_LOG" "$fixture/var/stdout" "$fixture/var/stderr"; do
    [[ $(< "$log") != *SECRET-ENROLLMENT-FIXTURE* && $(< "$log") != *PASSWORD-FIXTURE-NOT-IN-ARGV* && $(< "$log") != *'FAKE AGE IDENTITY'* && $(< "$log") != *TS-KEY-STDIN-ONLY* && $(< "$log") != *ENV-SECRET-FIXTURE* ]]
  done
}
run 2 install 0.1.0 --admin-name Owner
[[ $(< "$fixture/var/stderr") == *"dist/0.1.0/install.sh"* ]]
printf '#!/bin/bash\n# trusted fixture\n' > "$fixture/dist/0.1.0/install.sh"
printf '{"version":"0.1.0"}\n' > "$fixture/dist/0.1.0/manifest.json"
run 2 install 0.1.0 --admin-name Owner
[[ $(< "$fixture/var/stderr") == *manifest.json.minisig* ]]
printf 'fake signature\n' > "$fixture/dist/0.1.0/manifest.json.minisig"
printf 'FAKE AGE IDENTITY NEVER TRANSFER\n' > "$fixture/var/backup-recovery.key"
# No real TTY needed: pin rejection first, then emulate the terminal predicate only
# in the fixture copy to exercise interactive ssh argv without allocating a pty.
run 2 install 0.1.0 --admin-name Owner < /dev/null
[[ ! -s $SHIM_LOG && $(< "$fixture/var/stderr") == *--admin-password-file* ]]
python3 -I - "$fixture/scripts/deploy-server.sh" <<'PY'
import pathlib,sys
p=pathlib.Path(sys.argv[1]);s=p.read_text()
assert '[[ -t 0 && -t 1 ]]' in s
p.write_text(s.replace('[[ -t 0 && -t 1 ]]','[[ ${MOCK_LOCAL_TTY:-0} == 1 ]]'))
PY
export MOCK_LOCAL_TTY=1
# Wrapper fallback must work from another cwd without scripts/bin in PATH.
mkdir -p "$fixture/scripts/bin" "$fixture/elsewhere"
mv "$fixture/bin/age-keygen" "$fixture/scripts/bin/_tool"
for tool in age age-keygen minisign; do ln -s _tool "$fixture/scripts/bin/$tool"; done
(cd "$fixture/elsewhere"; run 0 install 0.1.0 --admin-name Owner)
python3 -I - "$SHIM_LOG" <<'PY'
import json,sys
rows=[json.loads(s) for s in open(sys.argv[1])]
assert rows[0]['tool']=='age-keygen','recipient must be derived before any remote call'
assert rows[0]['argv']==['-y','var/backup-recovery.key']
PY
mv "$fixture/scripts/bin/age-keygen" "$fixture/scripts/bin/age-keygen.disabled"
run 1 install 0.1.0 --admin-name Owner
[[ ! -s $SHIM_LOG && $(< "$fixture/var/stderr") == *age-keygen* ]]
mv "$fixture/scripts/bin/age-keygen.disabled" "$fixture/scripts/bin/age-keygen"
# Also cover the native-tool path (a regular executable, not a wrapper symlink).
cp "$fixture/scripts/bin/_tool" "$fixture/bin/age-keygen"
export STACK_EXISTS=1
run 1 install 0.1.0 --admin-name Owner
python3 -I - "$SHIM_LOG" <<'PY'
import json,shlex,sys
rows=[json.loads(s) for s in open(sys.argv[1])]
def no_install(rows):
 assert not any(r['tool']=='rsync' for r in rows)
 for r in rows:
  if r['tool']!='ssh': continue
  words=shlex.split(r['argv'][-1])
  assert 'server' not in words
  if words[:2]==['bash','-c']:
   # Nested scripts have newline-separated shell commands; inspect their words too.
   assert not any(w.endswith('/install.sh') and i+1<len(nested) and nested[i+1]=='server'
                  for nested in [shlex.split(words[2])] for i,w in enumerate(nested))
# Pin the assertion itself: direct and nested server invocations must be rejected.
for cmd in ["'bash' '/dist/install.sh' 'server' ",
            shlex.join(['bash','-c',"'bash' '/dist/install.sh' 'server' --root /stack"])]:
 try: no_install([{'tool':'ssh','argv':['offline-test-host',cmd]}])
 except AssertionError: pass
 else: raise AssertionError('no-install assertion missed server invocation')
no_install(rows)
PY
[[ $(< "$fixture/var/stderr") == *'production stack .env exists; use upgrade'* ]]
unset STACK_EXISTS
for status in 255 42 2; do
  export STACK_CHECK_STATUS=$status
  run 1 install 0.1.0 --admin-name Owner
  [[ $(< "$fixture/var/stderr") == *'cannot check remote stack state'* ]] || {
    printf 'FAIL stack probe exit %s needs transport diagnostic, not use upgrade\n' "$status" >&2; exit 1;
  }
  [[ $(< "$fixture/var/stderr") != *'use upgrade'* ]]
  python3 -I - "$SHIM_LOG" <<'PY'
import json,sys
rows=[json.loads(s) for s in open(sys.argv[1])]
assert [r['tool'] for r in rows]==['age-keygen','ssh','ssh'],'stack check failure must stop before mutation'
assert '# preflight' in rows[1]['argv'][-1] and '# stack-absent' in rows[2]['argv'][-1]
PY
done
unset STACK_CHECK_STATUS
printf 'PASS stack-exists versus failed remote stack probe\n'
run 0 install 0.1.0 --admin-name 'Owner display' --extra-san-dns home.example.ts.net
python3 -I - "$SHIM_LOG" <<'PY'
import json,shlex,sys
rows=[json.loads(s) for s in open(sys.argv[1])]
rs=[r for r in rows if r['tool']=='rsync'];assert len(rs)==1
assert rs[0]['argv'][:4]==['-a','--protect-args','--delete','-e']
assert rs[0]['argv'][-1]=='offline-test-host:/srv/data/porchlight-dist/0.1.0/'
commands=[shlex.split(r['argv'][-1]) for r in rows if r['tool']=='ssh']
assert ['mkdir','-p','-m','0750','--','/srv/data/porchlight-dist','/srv/data/porchlight-dist/0.1.0'] in commands
server=next(a for a in commands if 'server' in a)
assert server==['env','ENROLL_CIDRS=192.168.77.0/24','ENROLL_HOST=porchlight.lan:8444','PORCHLIGHT_ENDPOINTS=https://porchlight.lan:8443','PORCHLIGHT_GID=987','bash','/srv/data/porchlight-dist/0.1.0/install.sh','server','--source','/srv/data/porchlight-dist/0.1.0','--root','/srv/data/porchlight','--run-dir','/run/porchlight','--project','porchlight','--no-funnel','--backup-recipient','age1fixturepublicrecipient','--admin-name','Owner display','--san-dns','porchlight.lan','--san-dns','localhost','--san-ip','127.0.0.1','--san-ip','192.168.77.50','--san-dns','home.example.ts.net'],server
interactive=next(r for r in rows if r['tool']=='ssh' and 'server' in shlex.split(r['argv'][-1]));assert '-t' in interactive['argv']
assert all('-o' in r['argv'] and 'BatchMode=yes' in r['argv'] for r in rows if r['tool']=='ssh' and r is not interactive)
assert '# status' in rows[-1]['argv'][-1]
PY
printf 'PASSWORD-FIXTURE-NOT-IN-ARGV\n' > "$fixture/var/admin.pass"
chmod 600 "$fixture/var/admin.pass"
run 0 install 0.1.0 --admin-name "Owner's display; safe" --admin-password-file "$fixture/var/admin.pass"
python3 -I - "$SHIM_LOG" "$fixture/var/admin.pass" <<'PY'
import json,pathlib,shlex,subprocess,sys
text=pathlib.Path(sys.argv[1]).read_text();password=pathlib.Path(sys.argv[2]).read_text();rows=[json.loads(s) for s in text.splitlines()]
assert password.strip() not in text and 'FAKE AGE IDENTITY' not in text
sent=[r for r in rows if r.get('stdin_bytes',0)];assert len(sent)==1 and sent[0]['stdin_bytes']==len(password.encode())
assert '-T' in sent[0]['argv'] and '-t' not in sent[0]['argv'] and 'BatchMode=yes' in sent[0]['argv']
script=shlex.split(sent[0]['argv'][-1])[-1]
for fragment in ['mktemp','/srv/data/porchlight/tmp','chmod 600','trap','--admin-password-file','server']:
 assert fragment in script,fragment
# Run the emitted remote program locally with ONLY its fixed server paths remapped.
p=pathlib.Path(sys.argv[2]).parent.parent;remote=p/'remote';release=remote/'dist/0.1.0';release.mkdir(parents=True)
script=script.replace('/srv/data/porchlight-dist',str(remote/'dist')).replace('/srv/data/porchlight',str(remote/'stack'))
installer=release/'install.sh'
installer.write_text('''#!/usr/bin/env bash
set -euo pipefail
while [[ $1 != --admin-password-file ]]; do shift; done
[[ $(stat -c %a "$2") == 600 ]]
cmp -s "$2" "$FIXTURE/var/admin.pass"
[[ ${MOCK_INSTALL_FAIL:-0} == 0 ]]
''')
for failure in (False,True):
 import os
 env=os.environ.copy();env['MOCK_INSTALL_FAIL']=str(int(failure))
 r=subprocess.run(['bash','-c',script],input=password,text=True,capture_output=True,env=env)
 assert r.returncode==int(failure) and not r.stdout and not r.stderr
 assert not list((remote/'stack/tmp').iterdir()),'remote password file left behind'
print('PASS remote password staging and cleanup (success/failure)')
PY
export HASH_MISMATCH=1
run 1 install 0.1.0 --admin-name Owner
python3 -I - "$SHIM_LOG" <<'PY'
import json,shlex,sys
rows=[json.loads(s) for s in open(sys.argv[1])]
assert any(r['tool']=='rsync' for r in rows)
assert not any(r['tool']=='ssh' and 'server' in shlex.split(r['argv'][-1]) for r in rows)
PY
unset HASH_MISMATCH
export FAIL_CHECK=data-mount
run 1 preflight
[[ $(< "$fixture/var/stdout") == *'FAIL data-mount'* ]]
unset FAIL_CHECK
run 0 preflight
# Execute the actual emitted read-only preflight program against local mocked host tools.
# No real host paths are created/accessed: filesystem predicates use fixture directories.
python3 -I - "$SHIM_LOG" "$fixture" <<'PY'
import json,os,pathlib,shlex,subprocess,sys
p=pathlib.Path(sys.argv[2]);(p/'nixos').touch();(p/'run').mkdir();(p/'stack').mkdir()
program=shlex.split(json.loads(open(sys.argv[1]).readline())['argv'][-1])[-1]
program=program.replace('test -e /etc/NIXOS','test -e '+shlex.quote(str(p/'nixos')))
program=program.replace('private_dir /run/porchlight','private_dir '+shlex.quote(str(p/'run')))
assert "'root=/srv/data/porchlight'" in program and 'private_dir "$root"' in program,'configured stack root'
program=program.replace('check root-dir private_dir "$root"','check root-dir private_dir '+shlex.quote(str(p/'stack')))
assert 'check dist-dir' in program,'preflight must check incoming releases directory'
data=p/'data';data.mkdir();dist=data/'porchlight-dist'
program=program.replace('/srv/data/porchlight-dist',str(dist))
preamble=r'''
getent() { printf 'porchlight:x:987:stackuser\n'; }
id() { case $1 in -u) printf '1000\n' ;; -un) printf 'stackuser\n' ;; *) printf '%s\n' "${MOCK_GROUPS:-users docker porchlight}" ;; esac; }
stat() { if [[ $2 == %u ]]; then printf '%s\n' "${MOCK_DIST_OWNER:-1000}"; else printf '%s\n' "${MOCK_MODE:-stackuser:porchlight:750}"; fi; }
findmnt() { return "${MOCK_MOUNT_FAIL:-0}"; }
command() {
  if [[ ${1:-} == -v && ( $2 == minisign || $2 == age || $2 == rsync ) ]]; then [[ ${MOCK_MISSING_TOOL:-} != "$2" ]]; else builtin command "$@"; fi
}
systemctl() { printf 'RequiresMountsFor=%s\n' "${MOCK_MOUNT_ORDER:-/srv/data}"; }
docker() {
  case "$*" in
    info|'compose version') return 0 ;;
    'ps -a -q --filter label=com.docker.compose.project=porchlight') [[ -z ${MOCK_CONTAINERS:-} ]] || printf 'c0ffee\n'; return "${MOCK_PS_FAIL:-0}" ;;
    inspect*) printf '%s\n' "${MOCK_CONTAINERS:-}" ;;
    'compose ls -a --format json') printf '%s\n' "${MOCK_PROJECTS:-[]}" ;;
    *) return 1 ;;
  esac
}
ss() { printf '%s\n' "${MOCK_LISTENERS:-}"; }
export -f docker
'''
def probe(overrides,failed=None):
 env=os.environ.copy();env.update(overrides)
 r=subprocess.run(['bash','-c',preamble+program],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
 lines=r.stdout.splitlines();assert len(lines)==15 and all(s.startswith(('PASS ','FAIL ')) for s in lines),r.stdout
 assert r.returncode==(1 if failed else 0),(r.returncode,r.stdout,r.stderr)
 if failed: assert 'FAIL '+failed in lines,r.stdout
probe({}) # absent dist, writable parent
data.chmod(0o500);probe({},'dist-dir');data.chmod(0o700)
dist.touch();probe({},'dist-dir');dist.unlink()
dist.mkdir();probe({})
probe({'MOCK_DIST_OWNER':'0'},'dist-dir')
dist.chmod(0o500);probe({},'dist-dir');dist.chmod(0o700)
probe({'MOCK_MODE':'stackuser:users:750'},'run-dir')
# Ownership and membership are checked against the SSH login itself, never a configured account name.
probe({'MOCK_MODE':'someone-else:porchlight:750'},'run-dir')
probe({'MOCK_GROUPS':'users docker'},'group')
probe({'MOCK_MOUNT_FAIL':'1'},'data-mount')
probe({'MOCK_MISSING_TOOL':'age'},'age')
probe({'MOCK_MOUNT_ORDER':'/other'},'docker-mount')
probe({'MOCK_MOUNT_ORDER':'/srv/data-other'},'docker-mount')
probe({'MOCK_PS_FAIL':'1'},'compose-root')
probe({'MOCK_LISTENERS':'LISTEN 0 4096 0.0.0.0:8443 0.0.0.0:*'},'port-8443')
labels={'com.docker.compose.project':'porchlight','com.docker.compose.project.working_dir':'/srv/data/porchlight/releases/0.1.0','com.docker.compose.project.config_files':'/srv/data/porchlight/releases/0.1.0/compose.yml'}
container={'labels':labels,'running':True,'ports':{'8443/tcp':[{'HostIp':'0.0.0.0','HostPort':'8443'}]}}
probe({'MOCK_CONTAINERS':json.dumps(container),'MOCK_LISTENERS':'LISTEN 0 4096 0.0.0.0:8443 0.0.0.0:*'})
# One Porchlight binding must not excuse a different listener on the same port.
container['ports']['8443/tcp'][0]['HostIp']='192.168.77.50'
probe({'MOCK_CONTAINERS':json.dumps(container),'MOCK_LISTENERS':'LISTEN 0 4096 127.0.0.1:8443 0.0.0.0:*'},'port-8443')
container['ports']['8443/tcp'][0]['HostIp']='::'
probe({'MOCK_CONTAINERS':json.dumps(container),'MOCK_LISTENERS':'LISTEN 0 4096 [::]:8443 [::]:*'})
labels['com.docker.compose.project.working_dir']='/some/other/root'
probe({'MOCK_CONTAINERS':json.dumps(container)},'compose-root')
probe({'MOCK_PROJECTS':json.dumps([{'Name':'porchlight','ConfigFiles':'/other/compose.yml'}])},'compose-root')
print('PASS actual preflight checks with mocked host tools')
PY
run 0 enroll-uri --device 'Main laptop'
[[ $(stat -c %a "$fixture/var/m9") == 700 && $(stat -c %a "$fixture/var/m9/enroll-uri.json") == 600 ]]
[[ $(< "$fixture/var/stdout") == 'PASS enrollment URI saved to var/m9/enroll-uri.json' ]]
[[ $(< "$fixture/var/m9/enroll-uri.json") == *SECRET-ENROLLMENT-FIXTURE* ]]
python3 -I - "$SHIM_LOG" "$fixture" <<'PY'
import json,pathlib,shlex,subprocess,sys
p=pathlib.Path(sys.argv[2]);remote=p/'remote';stack=remote/'stack';release=stack/'releases/0.1.0'
release.mkdir(parents=True)
(stack/'init.json').write_text(json.dumps({'bootstrap_token':'FAKE-PRIVATE-PAT'}))
(stack/'release-current').symlink_to('releases/0.1.0')
cli=release/'porch-0.1.0-x86_64-linux-musl'
cli.write_text('''#!/usr/bin/env -S python3 -I
import json,os,pathlib,sys
args=sys.argv[1:];f=pathlib.Path(args[args.index('--token-file')+1])
assert f.stat().st_mode & 0o777 == 0o600
assert f.read_text() == 'FAKE-PRIVATE-PAT'
assert args[args.index('--socket')+1] == '/run/porchlight/agent.sock'
assert args[args.index('--user')+1] == 'admin'
assert args[args.index('--device')+1] == 'Main laptop'
if os.getenv('MOCK_CLI_FAIL')=='1':
 print(json.dumps({'type':'urn:porchlight:problem:client-error','title':'CLI request failed','status':None,'detail':'Could not connect to agent socket'}))
else: print(json.dumps({'uri':'porchlight://private-fixture'}))
sys.exit(int(os.getenv('MOCK_CLI_FAIL','0')))
''');cli.chmod(0o755)
script=shlex.split(json.loads(pathlib.Path(sys.argv[1]).read_text())['argv'][-1])[-1]
script=script.replace('/srv/data/porchlight',str(stack))
for failure in (False,True):
 import os
 env=os.environ.copy();env['MOCK_CLI_FAIL']=str(int(failure))
 r=subprocess.run(['bash','-c',script],capture_output=True,text=True,env=env)
 assert r.returncode==int(failure)
 assert not r.stderr,r.stderr  # Real porch --json errors go to stdout.
 assert 'FAKE-PRIVATE-PAT' not in r.stdout
 body=json.loads(r.stdout)
 if failure: assert body['detail']=='Could not connect to agent socket' and 'uri' not in body
 else: assert body['uri']=='porchlight://private-fixture'
 assert not list((stack/'tmp').iterdir()),'remote PAT file left behind'
print('PASS remote enrollment token handling and cleanup (success/failure)')
PY
# Pin exact bytes AND nanosecond mtime: identical replacement is still publication.
python3 -I - "$fixture/var/m9/enroll-uri.json" "$fixture/var/enroll-before.json" <<'PY'
import json,os,pathlib,sys
p=pathlib.Path(sys.argv[1]);os.utime(p,ns=(1_600_000_000_000_000_000,)*2)
pathlib.Path(sys.argv[2]).write_text(json.dumps({'bytes':p.read_bytes().hex(),'mtime_ns':p.stat().st_mtime_ns}))
PY
for failure in problem 1 malformed unsafe; do
  export ENROLL_FAIL=$failure
  run 1 enroll-uri --user missing-owner --device laptop
  python3 -I - "$fixture/var/m9/enroll-uri.json" "$fixture/var/enroll-before.json" "$fixture/var/stdout" "$fixture/var/stderr" "$failure" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]);before=json.loads(pathlib.Path(sys.argv[2]).read_text())
assert p.read_bytes().hex()==before['bytes'],'failed enrollment replaced output bytes'
assert p.stat().st_mtime_ns==before['mtime_ns'],'failed enrollment changed output mtime'
out=pathlib.Path(sys.argv[3]).read_text();err=pathlib.Path(sys.argv[4]).read_text()
assert not out,'failure stdout must stay unpublished'
assert 'porchlight://' not in err and 'FAILED-' not in err,'URI/token/raw/extension leaked'
if sys.argv[5]=='problem':
 assert 'Wrong user: requested member does not exist' in err,'stdout problem detail lost'
 assert 'Not found' in err and '404' in err and 'urn:porchlight:problem:not-found' in err
elif sys.argv[5]=='unsafe':
 assert 'safeline' in err,'sanitized detail missing'
 assert len(err)<500 and len(err.splitlines())==2,'unbounded/control problem output'
 assert not any(ord(c)<32 and c!='\n' for c in err),'control character leaked'
 assert 'False' not in err,'non-scalar/bool field rendered'
PY
  [[ $(ls -A "$fixture/var/m9") == enroll-uri.json ]]
done
unset ENROLL_FAIL
# Exercise real generated remote guards, with local fake CLI and no SSH host.
export MOCK_EXEC_ENROLL=1 MOCK_CLI_FAIL=1
run 1 enroll-uri --device 'Main laptop'
[[ $(< "$fixture/var/stderr") == *'Could not connect to agent socket'* ]]
unset MOCK_CLI_FAIL
printf 'FAKE-PRIVATE-PAT' > "$fixture/remote/stack/private.pat"
chmod 644 "$fixture/remote/stack/private.pat"
: > "$fixture/remote/stack/empty.pat"
chmod 600 "$fixture/remote/stack/empty.pat"
mkdir "$fixture/remote/stack/token-dir"
ln -s private.pat "$fixture/remote/stack/token-link"
for token in missing.pat private.pat empty.pat token-dir token-link; do
  run 1 enroll-uri --device 'Main laptop' --token-file "/srv/data/porchlight/$token"
  [[ $(< "$fixture/var/stderr") == *'token file must be a readable nonempty regular 0600 file (no symlinks)'* ]]
  [[ $(< "$fixture/var/stderr") != *FAKE-PRIVATE-PAT* ]]
  [[ ! -s $fixture/var/stdout && -z $(ls -A "$fixture/remote/stack/tmp") ]]
done
chmod 600 "$fixture/remote/stack/private.pat"
run 0 enroll-uri --device 'Main laptop' --token-file /srv/data/porchlight/private.pat --out var/m9/guard-ok.json
[[ $(< "$fixture/var/m9/guard-ok.json") == *porchlight://private-fixture* ]]
unset MOCK_EXEC_ENROLL
printf 'PASS enrollment stdout problem diagnostics, redaction, token-file guards and cleanup\n'
# Checkout aliases and output aliases must use physical paths consistently.
ln -s "$fixture" "$fixture/checkout-link"
# shellcheck disable=SC2030 # The checkout-alias override is intentionally confined to this subshell.
(fixture="$fixture/checkout-link"; run 0 enroll-uri --device laptop)
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
run 0 enroll-uri --device laptop --out "$fixture/checkout-link/var/m9/absolute.json"
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
ln -s m9 "$fixture/var/inside"
run 0 enroll-uri --device laptop --out var/inside/relative.json
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ -f $fixture/var/m9/absolute.json && -f $fixture/var/m9/relative.json ]]
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
ln -s "$fixture/elsewhere" "$fixture/var/escape"
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
for output in var/escape/enroll.json var/../elsewhere/enroll.json "$fixture/var-other/enroll.json"; do
  run 2 enroll-uri --device laptop --out "$output"
  # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
  [[ ! -s $SHIM_LOG && $(< "$fixture/var/stderr") == *'under this checkout var/'* ]]
done
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ ! -e $fixture/elsewhere/enroll.json && ! -e $fixture/var-other ]]
printf 'PASS physical checkout/output paths and var confinement\n'
run 0 enroll-uri --device 'Main laptop' --user owner --token-file /srv/data/porchlight/private.pat --out var/other/enroll.json
python3 -I - "$SHIM_LOG" <<'PY'
import json,shlex,sys
r=json.loads(open(sys.argv[1]).readline());s=shlex.split(r['argv'][-1])[-1]
for value in ['--socket','/run/porchlight/agent.sock','--token-file','--json','enroll','--user','owner','--device','Main laptop','chmod 600','trap']:
 assert value in s,value
PY
run 0 upgrade 0.1.0
python3 -I - "$SHIM_LOG" <<'PY'
import json,shlex,sys
rows=[json.loads(s) for s in open(sys.argv[1])]
assert any(r['tool']=='rsync' for r in rows)
commands=[shlex.split(r['argv'][-1]) for r in rows if r['tool']=='ssh']
upgrade=next(a for a in commands if 'upgrade' in a)
assert upgrade==['bash','/srv/data/porchlight-dist/0.1.0/install.sh','upgrade','--source','/srv/data/porchlight-dist/0.1.0','--root','/srv/data/porchlight','--run-dir','/run/porchlight','--project','porchlight']
assert '# status' in commands[-1][-1]
PY
run 0 status
python3 -I - "$SHIM_LOG" <<'PY'
import json,shlex,subprocess,sys
script=shlex.split(json.loads(open(sys.argv[1]).readline())['argv'][-1])[-1]
preamble='''
readlink() { printf '/srv/data/porchlight/releases/0.1.0\\n'; }
docker() { printf 'docker'; printf ' [%s]' "$@"; printf '\\n'; }
'''
r=subprocess.run(['bash','-c',preamble+script],capture_output=True,text=True)
assert r.returncode==0 and not r.stderr
assert 'Active release: /srv/data/porchlight/releases/0.1.0' in r.stdout
assert 'docker [compose] [ls] [-a] [--filter] [name=porchlight]' in r.stdout.splitlines(),r.stdout
assert len(r.stdout.splitlines())==4,'unexpected unscoped status command'
base='docker [compose] [-p] [porchlight] [--env-file] [/srv/data/porchlight/.env] [-f] [/srv/data/porchlight/releases/0.1.0/compose.yml] '
assert base+'[ps]' in r.stdout
assert base+'[exec] [-T] [api] [/usr/local/bin/porchlight] [healthcheck]' in r.stdout
print('PASS upgrade argv and actual status program (mocked Docker)')
PY
run 0 backup
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ $(< "$fixture/var/stdout") == 'PASS backup porchlight-20261007T030000000000Z.tar.age' ]]
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
python3 -I - "$SHIM_LOG" "$fixture" <<'PY'
import json,pathlib,shlex,subprocess,sys
p=pathlib.Path(sys.argv[2]);stack=p/'remote/stack';release=stack/'releases/0.1.0'
s=shlex.split(json.loads(open(sys.argv[1]).readline())['argv'][-1])[-1]
s=s.replace('/srv/data/porchlight',str(stack))
(release/'install.sh').write_text("#!/bin/bash\nprintf 'PRIVATE-BACKUP-OUTPUT\\n'\nprintf 'ERROR backup: unavailable database\\n' >&2\nexit 1\n")
r=subprocess.run(['bash','-c',s],capture_output=True,text=True)
assert r.returncode==1 and not r.stdout
assert 'ERROR backup: unavailable database' in r.stderr,r.stderr
assert 'PRIVATE-BACKUP-OUTPUT' not in r.stderr
assert not list((stack/'tmp').iterdir())
print('PASS remote backup failure diagnostics and cleanup')
PY
export PORCHLIGHT_DEPLOY_DRY_RUN=1
run 0 enroll-host 192.168.77.50
[[ ! -s $SHIM_LOG ]]
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
python3 -I - "$fixture/var/stdout" <<'PY'
import pathlib,shlex,sys
args=shlex.split(pathlib.Path(sys.argv[1]).read_text())
assert args==['DRY-RUN','ssh','-T','-o','BatchMode=yes','offline-test-host','bash','/srv/data/porchlight/release-current/install.sh','enroll-host','--host','192.168.77.50','--root','/srv/data/porchlight','--run-dir','/run/porchlight','--project','porchlight'],args
print('PASS enroll-host dry-run remote argv and deployment host')
PY
run 2 enroll-host
run 2 enroll-host 192.168.77.50 extra
for cmd in 'install 0.1.0 --admin-name Owner' 'upgrade 0.1.0' preflight status backup 'enroll-uri --device laptop'; do
  # Intentional word splitting of fixed, non-secret test command strings.
  read -r -a args <<< "$cmd"
  run 0 "${args[@]}"
  [[ ! -s $SHIM_LOG ]]
done
# Machine values never live in the repo: the environment, else the gitignored .env, else refuse (no defaults).
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
printf '# fixture profile\nPORCHLIGHT_DEPLOY_HOST=env-file-host\nPORCHLIGHT_DEPLOY_DATA_MOUNT=/srv/other\nPORCHLIGHT_DEPLOY_LAN=192.168.88.20/24\n' > "$fixture/.env"
(unset PORCHLIGHT_DEPLOY_HOST PORCHLIGHT_DEPLOY_DATA_MOUNT PORCHLIGHT_DEPLOY_LAN; run 0 install 0.1.0 --admin-name Owner)
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ $(< "$fixture/var/stdout") == *"ssh -t env-file-host "*"'ENROLL_CIDRS=192.168.88.0/24'"*"'/srv/other/porchlight-dist/0.1.0/install.sh'"*"'--san-ip' '192.168.88.20'"* ]]
run 0 install 0.1.0 --admin-name Owner   # exported variables beat the file
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ $(< "$fixture/var/stdout") == *"ssh -t offline-test-host "*"'ENROLL_CIDRS=192.168.77.0/24'"*"'/srv/data/porchlight-dist/0.1.0/install.sh'"* ]]
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
rm -- "$fixture/.env"
for key in PORCHLIGHT_DEPLOY_HOST PORCHLIGHT_DEPLOY_DATA_MOUNT PORCHLIGHT_DEPLOY_LAN; do
  (unset "$key"; run 2 install 0.1.0 --admin-name Owner)
  # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
  [[ $(< "$fixture/var/stderr") == *"set $key in .env"* ]]
done
(unset PORCHLIGHT_DEPLOY_LAN; run 2 funnel-check --fqdn quiet-node.example.ts.net)
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ $(< "$fixture/var/stderr") == *'set PORCHLIGHT_DEPLOY_LAN in .env'* ]]
for bad in /srv/data/ srv/data /srv/../etc; do
  (export PORCHLIGHT_DEPLOY_DATA_MOUNT=$bad; run 2 status)
  # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
  [[ $(< "$fixture/var/stderr") == *'must be an absolute path'* ]]
done
for bad in 192.168.77.50 192.168.77.50/32 not-an-address; do
  (export PORCHLIGHT_DEPLOY_LAN=$bad; run 2 install 0.1.0 --admin-name Owner)
  # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
  [[ $(< "$fixture/var/stderr") == *'invalid server LAN address'* ]]
done
printf 'PASS machine values from the environment or .env, never defaulted\n'
unset PORCHLIGHT_DEPLOY_DRY_RUN
run 2 install '../escape' --admin-name Owner
run 2 enroll-uri --device laptop --out dist/enroll.json
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
chmod 644 "$fixture/var/admin.pass"
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
run 2 install 0.1.0 --admin-name Owner --admin-password-file "$fixture/var/admin.pass"
printf 'PASS deploy-server offline contracts (install, preflight, secrets, hashes, enrollment, upgrade, status, backup, dry-run)\n'

# Funnel: execute the generated remote programs locally, never on an SSH server.
image=tailscale/tailscale@sha256:$(printf '%064d' 0)
fqdn=quiet-node.example.ts.net
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
key="$fixture/var/tailscale-authkey"
printf 'TS-KEY-STDIN-ONLY\n' > "$key"; chmod 600 "$key"
for hostname in '' Upper 'bad.name' 'bad;name' "$(printf 'a%.0s' {1..64})"; do
  run 2 funnel --ts-hostname "$hostname" --tailscale-image "$image"
  [[ ! -s $SHIM_LOG ]]
done
for ref in tailscale/tailscale:latest tailscale/tailscale@sha256:123 "${image%0}A" "evil/$image"; do
  run 2 funnel --ts-hostname quiet-node --tailscale-image "$ref"
  [[ ! -s $SHIM_LOG ]]
done
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
ln -s tailscale-authkey "$fixture/var/key-link"
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
mkdir "$fixture/var/key-dir"
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
: > "$fixture/var/key-empty"
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
chmod 600 "$fixture/var/key-empty"
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
for badkey in "$fixture/var/key-link" "$fixture/var/key-dir" "$fixture/var/key-empty" "$fixture/var/missing"; do
  run 2 funnel --ts-hostname quiet-node --tailscale-image "$image" --ts-authkey-file "$badkey"
  [[ ! -s $SHIM_LOG ]]
done
chmod 644 "$key"
run 2 funnel --ts-hostname quiet-node --tailscale-image "$image"
[[ ! -s $SHIM_LOG ]]
chmod 600 "$key"
# Emulate foreign ownership, without chown privileges.
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
cat > "$fixture/bin/stat" <<'SH'
#!/bin/bash
if [[ ${MOCK_FOREIGN_KEY:-0} == 1 && ${*: -1} == "$FIXTURE/var/tailscale-authkey" ]]; then
  printf '600:99999\n'
else exec /usr/bin/stat "$@"; fi
SH
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
chmod 755 "$fixture/bin/stat"
export MOCK_FOREIGN_KEY=1
run 2 funnel --ts-hostname quiet-node --tailscale-image "$image"
[[ ! -s $SHIM_LOG ]]
unset MOCK_FOREIGN_KEY
printf 'PASS Funnel local hostname/digest/key validation before transport\n'

# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
cat > "$fixture/remote/stack/releases/0.1.0/install.sh" <<'SH'
#!/bin/bash
set -euo pipefail
[[ $1 == funnel ]]; shift
stack="$FIXTURE/remote/stack"
mkdir -p "$stack/tailscale/state"
key=''
while (($#)); do
  case $1 in
    --root) [[ $2 == "$stack" ]] ;;
    --run-dir) [[ $2 == /run/porchlight ]] ;;
    --project) [[ $2 == porchlight ]] ;;
    --ts-hostname) [[ $2 == quiet-node ]] ;;
    --tailscale-image) [[ $2 == tailscale/tailscale@sha256:* ]] ;;
    --ts-authkey-file)
      key=$2
      [[ $key == "$stack/tmp/m9-ts-authkey" ]]
      [[ $(/usr/bin/stat -c %a "$key") == 600 ]]
      cmp -s "$key" "$FIXTURE/var/tailscale-authkey" ;;
    *) exit 9 ;;
  esac
  shift 2
done
printf 'Installer public SAN metadata\n'
printf 'Installer safe state diagnostic\n' >&2
# Model part A pre-staging failure and explicit staged-key/state retry semantics.
if [[ ${FUNNEL_RESULT:-good} == before-stage ]]; then
  printf 'Installed release lacks compose.funnel.yml; upgrade first\n' >&2; exit 1
fi
if [[ -n $key ]]; then cp "$key" "$stack/tailscale/authkey"; fi
[[ -s $stack/tailscale/authkey || -s $stack/tailscale/state/tailscaled.state ]] || {
  printf 'Funnel requires an auth key or existing node state\n' >&2; exit 1;
}
case ${FUNNEL_RESULT:-good} in
  good)
    printf 'node fixture\n' > "$stack/tailscale/state/tailscaled.state"
    rm -f "$stack/tailscale/authkey"
    printf 'FUNNEL_ENABLED=1\n' > "$stack/.env"
    printf 'PASS funnel quiet-node.example.ts.net\n' ;;
  fail)
    printf 'FUNNEL_ENABLED=0\nTS_AUTHKEY_SPEC=\n' > "$stack/.env"
    printf 'Funnel sidecar stopped and removed; leaving LAN-only (FUNNEL_ENABLED=0).\nStaged auth key retained privately; retry without --ts-authkey-file\n' >&2
    exit 1 ;;
  no-pass) : ;;
  bad-pass) printf 'PASS funnel not-a-fqdn\n' ;;
  non-ts-pass) printf 'PASS funnel porchlight.lan\n' ;;
  ip-pass) printf 'PASS funnel 1.2.3.4\n' ;;
  pass-fail) printf 'PASS funnel quiet-node.example.ts.net\n'; exit 1 ;;
esac
SH
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
chmod 755 "$fixture/remote/stack/releases/0.1.0/install.sh"
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
mkdir -p "$fixture/remote/stack/data/pki" "$fixture/remote/stack/tmp"
# Every activation exit removes deploy-server's transport, but never its laptop source.
for result in before-stage fail no-pass bad-pass non-ts-pass ip-pass pass-fail; do
  # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
  rm -rf "$fixture/remote/stack/tailscale"
  # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
  printf 'TRUNCATED-OLD-TRANSPORT\n' > "$fixture/remote/stack/tmp/m9-ts-authkey"
  export FUNNEL_RESULT=$result
  run 1 funnel --ts-hostname quiet-node --tailscale-image "$image"
  # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
  [[ ! -e $fixture/remote/stack/tmp/m9-ts-authkey && -f $key && ! -e $fixture/var/m9/funnel-fqdn ]]
  # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
  [[ $(< "$fixture/var/stderr") == *'Installer safe state diagnostic'* ]]
  # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
  [[ $(< "$fixture/var/stdout") == *'Installer public SAN metadata'* ]]
  if [[ $result == fail ]]; then
    # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
    [[ $(< "$fixture/var/stderr") == *'leaving LAN-only'* && $(< "$fixture/remote/stack/.env") == *FUNNEL_ENABLED=0* ]]
    # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
    cmp -s "$key" "$fixture/remote/stack/tailscale/authkey"
  fi
  # No source/state exists before staging: keyless retry must refuse, not get stuck on transport.
  if [[ $result == before-stage ]]; then
    unset FUNNEL_RESULT
    run 1 funnel --ts-hostname quiet-node --tailscale-image "$image" --no-authkey
    # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
    [[ $(< "$fixture/var/stderr") == *'Funnel requires an auth key or existing node state'* ]]
    run 0 funnel --ts-hostname quiet-node --tailscale-image "$image"
    # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
    rm -f "$fixture/var/m9/funnel-fqdn"
  fi
done
# A failed stdin write must clean both the private mktemp and any prior transport, before installer entry.
export STREAM_WRITE_FAIL=1
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
printf 'TRUNCATED-OLD-TRANSPORT\n' > "$fixture/remote/stack/tmp/m9-ts-authkey"
run 1 funnel --ts-hostname quiet-node --tailscale-image "$image"
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ ! -e $fixture/remote/stack/tmp/m9-ts-authkey && $(< "$fixture/var/stderr") != *'Installer safe state diagnostic'* ]]
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ $(ls -A "$fixture/remote/stack/tmp") == .m9-funnel.lock ]]
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ $(stat -c %a "$fixture/remote/stack/tmp/.m9-funnel.lock") == 600 ]]
unset STREAM_WRITE_FAIL
# A failure after staging retries without a new stdin key and consumes part A's staged copy.
export FUNNEL_RESULT=fail
run 1 funnel --ts-hostname quiet-node --tailscale-image "$image"
unset FUNNEL_RESULT
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
printf 'OLD-TRANSPORT\n' > "$fixture/remote/stack/tmp/m9-ts-authkey"
run 0 funnel --ts-hostname quiet-node --tailscale-image "$image" --no-authkey
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ ! -e $fixture/remote/stack/tmp/m9-ts-authkey && ! -e $fixture/remote/stack/tailscale/authkey ]]
run 0 funnel --ts-hostname quiet-node --tailscale-image "$image"
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ $(< "$fixture/var/stdout") == *"PASS funnel $fqdn"* ]]
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ $(< "$fixture/var/stdout") == *'delete the laptop auth key'* ]]
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ $(< "$fixture/var/m9/funnel-fqdn") == "$fqdn" && -f $key ]]
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ ! -e $fixture/remote/stack/tmp/m9-ts-authkey ]]
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ $(stat -c %a "$fixture/var/m9") == 700 && $(stat -c %a "$fixture/var/m9/funnel-fqdn") == 600 ]]
python3 -I - "$SHIM_LOG" "$key" <<'PY'
import json,pathlib,shlex,sys
rows=[json.loads(s) for s in open(sys.argv[1])]
assert len(rows)==1
assert rows[0]['stdin_bytes']==pathlib.Path(sys.argv[2]).stat().st_size
script=shlex.split(rows[0]['argv'][-1])[-1]
assert 'mktemp' in script and 'mv -Tf' in script and 'trap' in script and 'umask 077' in script and 'chmod 700' in script
assert '-T' in rows[0]['argv'] and '-t' not in rows[0]['argv'] and 'BatchMode=yes' in rows[0]['argv']
PY
# Explicit laptop path works from another cwd; input remains stdin only.
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
(cd "$fixture/elsewhere"; run 0 funnel --ts-hostname quiet-node --tailscale-image "$image" --ts-authkey-file "$key")
export FUNNEL_RESULT=fail
run 1 funnel --ts-hostname quiet-node --tailscale-image "$image" --no-authkey
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ $(< "$fixture/var/m9/funnel-fqdn") == "$fqdn" ]]
unset FUNNEL_RESULT
run 0 funnel --ts-hostname quiet-node --tailscale-image "$image" --no-authkey
python3 -I - "$SHIM_LOG" <<'PY'
import json,sys
rows=[json.loads(s) for s in open(sys.argv[1])]
assert len(rows)==1 and rows[0]['stdin_bytes']==0 and '--ts-authkey-file' not in rows[0]['argv'][-1]
PY
run 2 funnel --ts-hostname quiet-node --tailscale-image "$image" --no-authkey --ts-authkey-file "$key"
[[ ! -s $SHIM_LOG ]]
printf 'PASS Funnel atomic stdin-only transport, all-exit cleanup, safe diagnostics, staged-key retry and FQDN publication\n'

# Canned Docker output is consumed by the real assertions in the remote program.
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
cat > "$fixture/bin/docker" <<'PY'
#!/usr/bin/env -S python3 -I
import json,os,sys
args=sys.argv[1:];fqdn='quiet-node.example.ts.net';bad=os.getenv('FUNNEL_BAD','')
assert args[:4]==['compose','-p','porchlight','--env-file']
assert 'compose.funnel.yml' in ' '.join(args) and '--profile' in args
if args[-2:]==['config','-q']: sys.exit(0)
if args[-1]=='healthcheck': sys.exit(0)
if args[-2:]==['status','--json'] and 'serve' not in args:
 print(json.dumps({'BackendState':'Stopped' if bad=='running' else 'Running','Self':{'DNSName':fqdn+'.'}}))
elif 'sh' in args:
 assert 'test -z "$TS_AUTHKEY"' in args[-1] and '/config/authkey' in args[-1]
 sys.exit(1 if bad=='authkey' else 0)
elif 'serve' in args:
 print(json.dumps({'TCP':{'443':{'TCPForward':'api:8443' if bad=='serve' else 'api:9443','ProxyProtocol':1 if bad=='proxy' else 2}},'AllowFunnel':{fqdn+':443':bad!='allow'}}))
elif '--show' in args:
 print(json.dumps({'current':{'dns':['porchlight.lan','localhost']+([] if bad=='sans' else [fqdn]),'ips':['127.0.0.1','192.168.77.50']}}))
else: raise AssertionError('unexpected Docker operation')
PY
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
chmod 755 "$fixture/bin/docker"
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
python3 -I "$root/deploy/tests/funnel-tls.py" "$fixture"

export PORCHLIGHT_DEPLOY_DRY_RUN=1
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
rm -rf "$fixture/var/m9-funnel"
for auth in --no-authkey '--ts-authkey-file var/tailscale-authkey'; do
  read -r -a args <<< "$auth"
  run 0 funnel --ts-hostname quiet-node --tailscale-image "$image" "${args[@]}"
  # shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
  [[ ! -s $SHIM_LOG && ! -e $fixture/remote/stack/tmp/m9-ts-authkey ]]
done
run 0 funnel-check
# shellcheck disable=SC2031 # Use the parent fixture; the checkout-alias override was subshell-only.
[[ ! -s $SHIM_LOG && ! -e $fixture/var/m9-funnel ]]
unset PORCHLIGHT_DEPLOY_DRY_RUN
printf 'PASS Funnel dry-run executes no SSH/TLS probes or local state writes\n'
