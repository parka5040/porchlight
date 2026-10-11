#!/usr/bin/env bash
# Offline installer contract; only the optional final Compose config check uses real Docker.
set -euo pipefail
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
case ${1:-} in ''|--sandbox) ;; *) exit 2 ;; esac
python3 -I - "$root" <<'PY'
import json
import os
from pathlib import Path
import shutil
import shlex
import stat
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(sys.argv[1])
sys.argv = sys.argv[:1]
SERVE = b'{"TCP":{"443":{"TCPForward":"api:9443","ProxyProtocol":2}},"AllowFunnel":{"${TS_CERT_DOMAIN}:443":true}}'
IMAGE = 'tailscale/tailscale@sha256:' + 'a' * 64
FQDN = 'quiet-node.tail123.ts.net'
OLD = {'dns': ['porchlight.lan', 'localhost', 'extra.lan'], 'ips': ['127.0.0.1', '192.168.77.50']}
DOCKER = '''#!/usr/bin/env python3
import json,os,pathlib,signal,sys
p=pathlib.Path(os.environ['FIXTURE']);a=sys.argv[1:];root=pathlib.Path(os.environ['PORCHLIGHT_ROOT'])
envfile=root/'.env'
e=dict(x.split('=',1) for x in envfile.read_text().splitlines() if '=' in x) if envfile.exists() else {}
keys=['FUNNEL_ENABLED','TS_HOSTNAME','TAILSCALE_IMAGE_SHA256','TS_AUTHKEY_SPEC','PORCHLIGHT_ENDPOINTS','ENROLL_HOST','ENROLL_PORT']
record={'argv':a,'env':{k:e[k] for k in keys if k in e},'authkey':(root/'tailscale/authkey').exists(),
        'inherited':[k for k in keys+['TS_AUTHKEY','FUNNEL_IP'] if k in os.environ],
        'state_dirs':[[x.stat().st_mode & 0o777,x.stat().st_uid] if x.is_dir() else None for x in [root/'tailscale',root/'tailscale/state']]}
with (p/'docker.jsonl').open('a') as f: f.write(json.dumps(record)+'\\n')
if a in (['info'],['compose','version']) or a[0] in ('load','ps'): sys.exit(0)
assert a[0]=='compose'
if a[-2:]==['backup','now']:
 name='porchlight-20261007T120000000000Z.tar.age'
 (root/'backups').mkdir(exist_ok=True);(root/'backups'/name).write_text('fixture bundle')
 print('PASS backup '+name)
cleanup=a[-3:]==['rm','-sf','tailscale'] or ('up' in a and a[-1]=='api' and '--force-recreate' not in a)
if cleanup and os.environ.get('CLEANUP_SIGNALS'):
 ignored=[signal.getsignal(s)==signal.SIG_IGN for s in (signal.SIGHUP,signal.SIGINT,signal.SIGTERM)]
 with (p/'cleanup-signals').open('a') as f: f.write(json.dumps(ignored)+'\\n')
 # Every fixture installer has its own process group; never signal the test runner.
 for s in (signal.SIGHUP,signal.SIGINT,signal.SIGTERM): os.killpg(os.getpgrp(),s)
if 'up' in a and a[-1] in ('api','tailscale','backup'):
 service=a[-1]
 if service=='tailscale' and os.environ.get('FAIL_START'): sys.exit(1)
 if service=='api' and '--force-recreate' not in a and os.environ.get('FAIL_RECOVERY'): sys.exit(1)
 if '--force-recreate' in a and os.environ.get('FAIL_RECREATE')==service: sys.exit(1)
 if '--force-recreate' in a and os.environ.get('FAIL_ENROLL_ONCE')==service and not (p/'failed-enroll').exists():
  (p/'failed-enroll').write_text('1');sys.exit(1)
 (p/(service+'-running')).write_text('1')
if a[-2:]==['stop','api']: (p/'api-running').write_text('0')
if a[-3:]==['rm','-sf','tailscale']: (p/'tailscale-running').write_text('0')
if 'pg_restore' in a and os.environ.get('FAIL_RESTORE'): sys.exit(1)
if a[-4:]==['tailscale','tailscale','status','--json']:
 counter=p/'status-count';n=int(counter.read_text()) if counter.exists() else 0;counter.write_text(str(n+1))
 if n==0 and os.environ.get('LOGIN_PENDING'):
  print(json.dumps({'BackendState':'NeedsLogin'}))
 else:
  (root/'tailscale/state/tailscaled.state').write_text('fixture-node-state')
  peer={'DNSName':os.environ.get('DNS_NAME','quiet-node.tail123.ts.net.')}
  fields=os.environ.get('CAP_FIELDS_FIRST') if n==0 and os.environ.get('CAP_FIELDS_FIRST') else os.environ.get('CAP_FIELDS','{"CapMap":{"https":[],"funnel":[],"https://tailscale.com/cap/funnel-ports?ports=443,8443,10000":[]}}')
  peer.update(json.loads(fields))
  print(json.dumps({'BackendState':'Running','Self':peer}))
if a[-3:]==['serve','status','--json']:
 counter=p/'serve-count';n=int(counter.read_text()) if counter.exists() else 0;counter.write_text(str(n+1))
 config={'TCP':{'443':{'TCPForward':'api:9443','ProxyProtocol':2}},'AllowFunnel':{'quiet-node.tail123.ts.net:443':True}}
 bad=os.environ.get('BAD_SERVE') if not os.environ.get('SERVE_PENDING') or n==0 else ''
 if bad=='allow': config['AllowFunnel']={}
 if bad=='target': config['TCP']['443']['TCPForward']='other:9443'
 if bad=='proxy': config['TCP']['443']['ProxyProtocol']=1
 print(json.dumps(config))
if 'server-cert' in a:
 sans=p/'sans.json'
 if '--show' in a:
  if os.environ.get('FAIL_SHOW'):
   print('cannot read certificate: fixture reason',file=sys.stderr);sys.exit(1)
  print(json.dumps({'current':json.loads(sans.read_text()),'previous':{},'not_after':'2028-10-01T00:00:00Z'}))
 else:
  if os.environ.get('FAIL_CERT'):
   print('invalid DNS SAN: fixture reason',file=sys.stderr);sys.exit(1)
  dns=[a[i+1] for i,x in enumerate(a) if x=='--san-dns'];ips=[a[i+1] for i,x in enumerate(a) if x=='--san-ip']
  sans.write_text(json.dumps({'dns':dns,'ips':ips}))
  print('previous SANs: fixture; current SANs: fixture; not_after: 2028-10-01T00:00:00Z')
'''

class InstallerTests(unittest.TestCase):
    def setUp(self):
        (ROOT/'var/tests').mkdir(parents=True, exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT/'var/tests', prefix='funnel.')
        self.addCleanup(self.tmp.cleanup)
        self.p = Path(self.tmp.name)
        self.stack = self.p/'stack with spaces'
        self.release = self.stack/'releases/0.1.0'
        self.release.mkdir(parents=True)
        for name in ('compose.yml', 'compose.funnel.yml'):
            shutil.copyfile(ROOT/'deploy'/name, self.release/name)
        (self.stack/'release-current').symlink_to('releases/0.1.0')
        (self.stack/'data/pki').mkdir(parents=True)
        (self.stack/'.env').write_text('PORCHLIGHT_VERSION=0.1.0\nPORCHLIGHT_ENDPOINTS=https://porchlight.lan:8443\n')
        (self.stack/'.env').chmod(0o600)
        self.installer = self.p/'install.sh'
        subprocess.run(['python3', '-I', str(ROOT/'scripts/build-installer.py'), str(ROOT), str(self.installer), 'TEST-PUBLIC-KEY'], check=True)
        bindir = self.p/'bin'; bindir.mkdir()
        (bindir/'docker').write_text(DOCKER); (bindir/'docker').chmod(0o755)
        (bindir/'timeout').write_text('''#!/usr/bin/env python3
import os,pathlib,sys
p=pathlib.Path(os.environ['FIXTURE'])
with (p/'timeouts').open('a') as f: f.write(sys.argv[3 if '--signal=KILL' in sys.argv else 2]+'\\n')
with (p/'timeout-signals').open('a') as f: f.write(str('--signal=KILL' in sys.argv)+'\\n')
if os.environ.get('TIMEOUT_STATUS') and sys.argv[-3:]==['tailscale','status','--json']: sys.exit(int(os.environ['TIMEOUT_STATUS']))
if os.environ.get('BAD_SERVE') and not os.environ.get('SERVE_PENDING') and (p/'serve-count').exists() and sys.argv[-3:]==['serve','status','--json']: sys.exit(137)
os.execv('''+repr(shutil.which('timeout'))+''', ['timeout',*sys.argv[1:]])
''')
        (bindir/'timeout').chmod(0o755)
        (bindir/'sleep').write_text('#!/bin/sh\nexit 0\n'); (bindir/'sleep').chmod(0o755)
        (bindir/'minisign').write_text('#!/bin/sh\nexit 0\n'); (bindir/'minisign').chmod(0o755)
        self.key = self.p/'input-key'
        self.key.write_text('fixture-only-not-a-real-key\n'); self.key.chmod(0o600)
        (self.p/'sans.json').write_text(json.dumps(OLD))
        self.env = dict(os.environ, PATH=str(bindir)+':'+os.environ['PATH'], FIXTURE=str(self.p),
                        HOME=str(self.p/'home'), XDG_STATE_HOME=str(self.p/'state'),
                        XDG_DATA_HOME=str(self.p/'desktop'), XDG_CONFIG_HOME=str(self.p/'config'),
                        PORCHLIGHT_FUNNEL_CAP_GRACE_SECS='0')
        for k in ('FUNNEL_ENABLED','TS_HOSTNAME','TAILSCALE_IMAGE_SHA256','TS_AUTHKEY_SPEC','TS_AUTHKEY','FUNNEL_IP','PORCHLIGHT_ENDPOINTS'):
            self.env[k] = 'hostile-caller-value'

    def run_install(self, mode, *args, ok=True, env=None, stack=None, closed_output=False):
        prefix = [mode]
        if mode=='restore': prefix.append(args[0]); args=args[1:]
        command = ['bash', str(self.installer), *prefix, '--root', str(stack or self.stack),
                   '--run-dir', str(self.p/'run'), '--project', 'porchlight-fixture', *args]
        if closed_output:
            reader, writer = os.pipe()
            os.close(reader)
            try:
                result = subprocess.run(command, env=dict(self.env, **(env or {})), stdout=writer, stderr=writer,
                                        stdin=subprocess.DEVNULL, timeout=30, start_new_session=True)
            finally:
                os.close(writer)
            result.stdout = result.stderr = ''
        else:
            result = subprocess.run(command, env=dict(self.env, **(env or {})), capture_output=True,
                                    text=True, stdin=subprocess.DEVNULL, timeout=30, start_new_session=True)
        if ok is not None:
            self.assertEqual(result.returncode == 0, ok, f'{mode} exit={result.returncode}: {result.stderr[:500]}')
        for text in (result.stdout, result.stderr, (self.p/'docker.jsonl').read_text() if (self.p/'docker.jsonl').exists() else ''):
            self.assertNotIn(self.key.read_text().strip(), text)
        return result

    def funnel(self, *args, **kwargs):
        return self.run_install('funnel', '--ts-hostname', 'quiet-node', '--tailscale-image', IMAGE, *args, **kwargs)

    def records(self):
        path = self.p/'docker.jsonl'
        return [json.loads(x) for x in path.read_text().splitlines()] if path.exists() else []

    def assert_only_diagnostics(self):
        # Failed activation keeps sanitized evidence, never the secret-bearing install stage.
        directories = list((self.stack/'tmp').iterdir())
        self.assertTrue(directories)
        for directory in directories:
            self.assertTrue(directory.name.startswith('failure.'))
            self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
            self.assertEqual([p.name for p in directory.iterdir()], ['summary.txt'])
            summary = directory/'summary.txt'
            self.assertEqual(summary.stat().st_mode & 0o777, 0o600)
            self.assertNotIn(self.key.read_text().strip(), summary.read_text())

    def env_values(self):
        return dict(x.split('=', 1) for x in (self.stack/'.env').read_text().splitlines() if '=' in x)

    def test_enroll_host_valid_hosts_ports_and_idempotence(self):
        for host, expected in [('192.168.77.50', '192.168.77.50:8444'),
                               ('porchlight.lan', 'porchlight.lan:8444'),
                               ('localhost:1', 'localhost:1'), ('192.168.77.50:65535', '192.168.77.50:65535')]:
            with self.subTest(host=host):
                result = self.run_install('enroll-host', '--host', host)
                self.assertIn('PASS enroll-host '+expected, result.stdout)
                self.assertEqual(self.env_values()['ENROLL_HOST'], expected)
                self.assertEqual(self.env_values()['PORCHLIGHT_ENDPOINTS'], 'https://porchlight.lan:8443')
                self.assertEqual(json.loads((self.p/'sans.json').read_text()), OLD)
                before = len(self.records())
                again = self.run_install('enroll-host', '--host', host)
                self.assertIn('PASS enroll-host '+expected, again.stdout)
                self.assertFalse(any('up' in r['argv'] for r in self.records()[before:]))
        calls = [r for r in self.records() if 'up' in r['argv']]
        self.assertEqual([r['argv'][-1] for r in calls], ['api', 'backup'] * 4)
        for r in calls:
            self.assertIn('--force-recreate', r['argv']); self.assertIn('--wait', r['argv'])
        self.assertEqual(stat.S_IMODE((self.stack/'.env').stat().st_mode), 0o600)
        self.assertFalse(list((self.stack/'tmp').iterdir()))

    def test_enroll_host_funnel_enabled_recreate_and_recovery(self):
        for failure, services in [(None, ['api', 'backup']),
                                  ('api', ['api', 'api', 'backup']),
                                  ('backup', ['api', 'backup', 'api', 'backup'])]:
            with self.subTest(failure=failure):
                (self.stack/'.env').write_text(
                    'PORCHLIGHT_VERSION=0.1.0\nPORCHLIGHT_ENDPOINTS=https://porchlight.lan:8443,https='+FQDN+'\n'
                    'ENROLL_HOST=porchlight.lan:8444\nFUNNEL_ENABLED=1\nTS_HOSTNAME=quiet-node\n'
                    'TAILSCALE_IMAGE_SHA256='+('a'*64)+'\n')
                before = (self.stack/'.env').read_bytes()
                (self.p/'docker.jsonl').unlink(missing_ok=True)
                (self.p/'failed-enroll').unlink(missing_ok=True)
                result = self.run_install('enroll-host', '--host', '192.168.77.50',
                                          ok=failure is None, env={'FAIL_ENROLL_ONCE':failure} if failure else {})
                calls = [r for r in self.records() if 'up' in r['argv']]
                self.assertEqual([r['argv'][-1] for r in calls], services)
                for r in calls:
                    self.assertIn(str(self.release/'compose.funnel.yml'), r['argv'])
                    profile = r['argv'].index('--profile')
                    self.assertEqual(r['argv'][profile+1], 'funnel')
                    self.assertIn('--force-recreate', r['argv'])
                self.assertFalse(any('tailscale' in r['argv'] for r in self.records()))
                if failure:
                    self.assertIn('previous ENROLL_HOST restored; API recovery succeeded.', result.stderr)
                    self.assertEqual((self.stack/'.env').read_bytes(), before)
                    self.assertTrue(all(r['env']['ENROLL_HOST']=='porchlight.lan:8444' for r in calls[-2:]))
                    for r, wait in zip(calls[-2:], ['30', '120'], strict=True):
                        self.assertEqual(r['argv'][r['argv'].index('--wait-timeout')+1], wait)
                    self.assertEqual((self.p/'timeouts').read_text().splitlines()[-2:], ['30', '120'])
                else:
                    self.assertIn('PASS enroll-host 192.168.77.50:8444', result.stdout)
                    self.assertTrue(all(r['env']['ENROLL_HOST']=='192.168.77.50:8444' for r in calls))
                self.assertEqual((self.p/'api-running').read_text(), '1')
                self.assertEqual((self.p/'backup-running').read_text(), '1')

    def test_enroll_host_default_uses_stack_port_not_caller(self):
        with (self.stack/'.env').open('a') as f: f.write('ENROLL_PORT=9444\n')
        result = self.run_install('enroll-host', '--host', '192.168.77.50',
                                  env={'ENROLL_PORT':'evil', 'ENROLL_HOST':'evil'})
        self.assertIn('PASS enroll-host 192.168.77.50:9444', result.stdout)
        self.assertFalse(any('ENROLL_HOST' in r['inherited'] or 'ENROLL_PORT' in r['inherited'] for r in self.records()))

    def test_enroll_host_rejects_before_staging_or_docker(self):
        before = (self.stack/'.env').read_bytes()
        for host in ['', 'bad name', '-bad.lan', 'bad..lan', 'bad_.lan', 'https://porchlight.lan',
                     'porchlight.lan/path', 'user@porchlight.lan', 'porchlight.lan\nX=bad',
                     '192.168.77.999', '192.168.77', '192.168.077.50', '::1', '[::1]:8444',
                     'porchlight.lan:', 'porchlight.lan:abc', 'porchlight.lan:0', 'porchlight.lan:65536']:
            with self.subTest(host=host):
                self.run_install('enroll-host', '--host', host, ok=False)
                self.assertEqual(self.records(), [])
                self.assertFalse((self.stack/'tmp').exists())
                self.assertEqual((self.stack/'.env').read_bytes(), before)

    def test_enroll_host_requires_current_san_without_changes(self):
        before = (self.stack/'.env').read_bytes()
        for host in ['not-in-san.lan', '192.168.77.51']:
            result = self.run_install('enroll-host', '--host', host, ok=False)
            self.assertIn('install.sh server-cert', result.stderr)
            self.assertEqual((self.stack/'.env').read_bytes(), before)
        self.assertFalse(any('up' in r['argv'] or 'stop' in r['argv'] for r in self.records()))
        self.assert_only_diagnostics()

    def test_enroll_host_show_failure_and_invalid_metadata_leave_stack_unchanged(self):
        before = (self.stack/'.env').read_bytes()
        result = self.run_install('enroll-host', '--host', '192.168.77.50', ok=False, env={'FAIL_SHOW':'1'})
        self.assertIn('ENROLL_HOST unchanged', result.stderr)
        (self.p/'sans.json').write_text('{"dns":false}')
        self.run_install('enroll-host', '--host', '192.168.77.50', ok=False)
        self.assertEqual((self.stack/'.env').read_bytes(), before)
        self.assertFalse(any('up' in r['argv'] for r in self.records()))
        self.assert_only_diagnostics()

    def test_enroll_host_no_backup_and_closed_output_complete_change(self):
        self.run_install('enroll-host', '--host', '192.168.77.50', '--no-backup', closed_output=True)
        self.assertEqual(self.env_values()['ENROLL_HOST'], '192.168.77.50:8444')
        self.assertEqual([r['argv'][-1] for r in self.records() if 'up' in r['argv']], ['api'])
        self.assertFalse(list((self.stack/'tmp').iterdir()))

    def test_enroll_host_failed_recreate_restores_previous_and_recovers(self):
        for service in ['api', 'backup']:
            with self.subTest(service=service):
                with (self.stack/'.env').open('a') as f: f.write('ENROLL_HOST=porchlight.lan:8444\n')
                before = (self.stack/'.env').read_bytes()
                (self.p/'failed-enroll').unlink(missing_ok=True)
                result = self.run_install('enroll-host', '--host', '192.168.77.50', ok=False,
                                          env={'FAIL_ENROLL_ONCE':service})
                self.assertNotIn('PASS enroll-host', result.stdout)
                self.assertIn('previous ENROLL_HOST restored', result.stderr)
                self.assertEqual((self.stack/'.env').read_bytes(), before)
                self.assertEqual((self.p/'api-running').read_text(), '1')
                last_api = [r for r in self.records() if 'up' in r['argv'] and r['argv'][-1]=='api'][-1]
                self.assertEqual(last_api['env']['ENROLL_HOST'], 'porchlight.lan:8444')
                self.assertIn('--force-recreate', last_api['argv'])
                self.assert_only_diagnostics()

    def test_enroll_host_failed_recovery_reports_once(self):
        with (self.stack/'.env').open('a') as f: f.write('ENROLL_HOST=porchlight.lan:8444\n')
        before = (self.stack/'.env').read_bytes()
        result = self.run_install('enroll-host', '--host', '192.168.77.50', ok=False,
                                  env={'FAIL_RECREATE':'api'})
        self.assertEqual((self.stack/'.env').read_bytes(), before)
        self.assertIn('previous ENROLL_HOST restored; API recovery failed; retry API recreation.', result.stderr)
        self.assertEqual(result.stderr.count('Recovery command:'), 1)
        command = next(line.partition(': ')[2] for line in result.stderr.splitlines()
                       if line.startswith('Recovery command:'))
        self.assertEqual(shlex.split(command)[-3:], ['--wait-timeout', '30', 'api'])
        self.assertEqual(result.stderr.count('API recovery failed'), 1)
        self.assertNotIn('start api with the installed Compose files', result.stderr)

    def test_enable_and_rerun_preserve_sans_and_clear_key(self):
        result = self.funnel('--ts-authkey-file', str(self.key), env={'LOGIN_PENDING':'1'})
        self.assertIn('PASS funnel '+FQDN, result.stdout)
        ts = self.stack/'tailscale'
        self.assertEqual((ts/'serve.json').read_bytes(), SERVE)
        for path, mode in [(ts,0o700),(ts/'state',0o700),(ts/'serve.json',0o600),(self.stack/'.env',0o600)]:
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), mode)
        self.assertFalse((ts/'authkey').exists())
        values = self.env_values()
        self.assertEqual(values['FUNNEL_ENABLED'], '1')
        self.assertEqual(values['TS_HOSTNAME'], 'quiet-node')
        self.assertEqual(values['TAILSCALE_IMAGE_SHA256'], 'a'*64)
        self.assertEqual(values['TS_AUTHKEY_SPEC'], '')
        self.assertEqual(values['PORCHLIGHT_ENDPOINTS'], 'https://porchlight.lan:8443,https://'+FQDN)
        records = [x for x in self.records() if '--env-file' in x['argv']]
        for x in records:
            self.assertIn(str(self.release/'compose.funnel.yml'), x['argv'])
            self.assertIn('--profile', x['argv'])
            self.assertEqual(x['inherited'], [])
        pending = next(x for x in records if x['argv'][-3:]==['tailscale','status','--json'])
        self.assertTrue(pending['authkey'])  # first canned status is NeedsLogin
        self.assertEqual(pending['env']['TS_AUTHKEY_SPEC'], 'file:/config/authkey')
        self.assertTrue(records[0]['authkey'])
        self.assertEqual(records[0]['env']['TS_AUTHKEY_SPEC'], 'file:/config/authkey')
        recreated = [x for x in records if '--force-recreate' in x['argv'] and x['argv'][-1]=='tailscale']
        self.assertEqual(len(recreated), 1)
        self.assertFalse(recreated[0]['authkey'])
        self.assertEqual(recreated[0]['env']['TS_AUTHKEY_SPEC'], '')
        calls = [x['argv'] for x in records]
        show = next(i for i,a in enumerate(calls) if '--show' in a)
        stop = next(i for i,a in enumerate(calls) if a[-2:]==['stop','api'])
        cert = next(i for i,a in enumerate(calls) if 'server-cert' in a and '--show' not in a)
        start = next(i for i,a in enumerate(calls) if 'up' in a and a[-1]=='api')
        self.assertLess(show, stop); self.assertLess(stop, cert); self.assertLess(cert, start)
        self.assertIn('--force-recreate', calls[start])
        self.assertEqual(records[start]['env']['PORCHLIGHT_ENDPOINTS'], values['PORCHLIGHT_ENDPOINTS'])
        backup = next(i for i,a in enumerate(calls) if 'up' in a and a[-1]=='backup')
        self.assertLess(start, backup)
        self.assertIn('--force-recreate', calls[backup])  # .env bind must follow its atomic replacement
        self.assertEqual(records[backup]['env']['PORCHLIGHT_ENDPOINTS'], values['PORCHLIGHT_ENDPOINTS'])
        self.assertTrue(all(0 < int(x) <= 120 for x in (self.p/'timeouts').read_text().splitlines()))
        sans = json.loads((self.p/'sans.json').read_text())
        self.assertEqual(set(sans['dns']), set(OLD['dns']+[FQDN]))
        self.assertEqual(set(sans['ips']), set(OLD['ips']))
        before = (self.stack/'.env').read_bytes()
        (self.p/'docker.jsonl').unlink()
        self.funnel()
        self.assertEqual((self.stack/'.env').read_bytes(), before)
        self.assertEqual(json.loads((self.p/'sans.json').read_text()), sans)
        self.assertTrue(all(not x['authkey'] for x in self.records()))

    def assert_disabled(self, result=None, changed=False):
        if result is not None:
            self.assertIn('sidecar stopped and removed', result.stderr)
        values = self.env_values()
        self.assertEqual(values['FUNNEL_ENABLED'], '0')
        self.assertEqual(values['TS_AUTHKEY_SPEC'], '')
        self.assertEqual((self.p/'tailscale-running').read_text(), '0')
        if not changed:
            self.assertEqual(values['PORCHLIGHT_ENDPOINTS'], 'https://porchlight.lan:8443')
            self.assertEqual(json.loads((self.p/'sans.json').read_text()), OLD)
        removed = [x for x in self.records() if x['argv'][-3:]==['rm','-sf','tailscale']]
        self.assertEqual(len(removed), 1)
        self.assertIn('--profile', removed[0]['argv'])
        self.run_install('backup')
        self.assertNotIn('--profile', self.records()[-1]['argv'])

    def test_failed_login_disables_and_removes_sidecar(self):
        for failure in ({'FAIL_START':'1'}, {'TIMEOUT_STATUS':'124'}, {'DNS_NAME':'bad name.ts.net.'}):
            with self.subTest(failure=failure):
                with (self.stack/'.env').open('a') as f: f.write('FUNNEL_ENABLED=1\n')
                (self.p/'docker.jsonl').unlink(missing_ok=True)
                result = self.funnel('--ts-authkey-file', str(self.key), env=failure, ok=False)
                self.assert_disabled(result)
                self.assertEqual(stat.S_IMODE((self.stack/'tailscale/authkey').stat().st_mode), 0o600)

    def test_closed_pipe_failed_login_disables_without_output(self):
        self.funnel('--ts-authkey-file', str(self.key), env={'FAIL_START':'1'}, closed_output=True, ok=False)
        self.assert_disabled()
        self.assertEqual(stat.S_IMODE((self.stack/'tailscale/authkey').stat().st_mode), 0o600)
        self.assert_only_diagnostics()

    def test_closed_pipe_successful_rotation_recovers_api(self):
        self.funnel('--ts-authkey-file', str(self.key), closed_output=True)
        self.assertEqual(self.env_values()['FUNNEL_ENABLED'], '1')
        self.assertEqual(self.env_values()['PORCHLIGHT_ENDPOINTS'], 'https://porchlight.lan:8443,https://'+FQDN)
        self.assertIn(FQDN, json.loads((self.p/'sans.json').read_text())['dns'])
        self.assertEqual((self.p/'api-running').read_text(), '1')
        calls = [x['argv'] for x in self.records()]
        stop = next(i for i,a in enumerate(calls) if a[-2:]==['stop','api'])
        self.assertTrue(any('up' in a and a[-1]=='api' for a in calls[stop+1:]))
        self.assertEqual(self.env_values()['TS_AUTHKEY_SPEC'], '')
        self.assertFalse((self.stack/'tailscale/authkey').exists())
        self.assertEqual(list((self.stack/'tmp').iterdir()), [])

    def test_status_capabilities_present_absent_old_format_and_malformed(self):
        https, funnel = 'https', 'funnel'
        ports = 'https://tailscale.com/cap/funnel-ports?ports=443,8443,10000'
        caps = [https, funnel, ports]
        sentinel = 'HTTPS://TAILSCALE.COM/s/DEPRECATED-NODE-CAPS#see-cap-map'
        cases = [({'CapMap':dict.fromkeys(caps, []), 'Capabilities':[sentinel]}, None),
                 ({'CapMap':dict.fromkeys(caps)}, None),
                 ({'Capabilities':caps}, None),
                 ({'CapMap':None, 'Capabilities':caps}, None),
                 ({'Capabilities':[https,funnel,ports.replace('443,8443,10000','400-500')]}, None),
                 ({'CapMap':dict.fromkeys([https,funnel,ports.replace('443,8443,10000','400-500')], [])}, None),
                 ({'Capabilities':[https,funnel,ports.replace('443,8443,10000','443%2C8443')]}, None),
                 ({}, 'https capability missing'), ({'CapMap':{}}, 'https capability missing'),
                 ({'Capabilities':[]}, 'https capability missing'),
                 ({'CapMap':{https:[],funnel:[]}, 'Capabilities':[ports]}, 'funnel-ports capability'),
                 ({'CapMap':{}, 'Capabilities':caps}, 'https capability missing'),
                 ({'Capabilities':[https,'https://tailscale.com/cap/funnel',ports]}, 'funnel capability missing')]
        for missing, message in zip(caps, ('https capability missing','funnel capability missing','funnel-ports capability')):
            cases += [({'CapMap':{k:[] for k in caps if k!=missing}}, message),
                      ({'Capabilities':[k for k in caps if k!=missing]}, message)]
        cases += [({'CapMap':caps}, 'capability data malformed'), ({'Capabilities':ports}, 'capability data malformed'),
                  ({'Capabilities':[https,funnel,443]}, 'capability data malformed'),
                  ({'CapMap':443}, 'capability data malformed')]
        for bad in ('8443,10000','443x','4443','450-440','443-','0-70000','443&ports=8443',
                    '', '443,', '443&extra=1', '443#fragment', '443%', '443%ZZ'):
            cases.append(({'Capabilities':[https,funnel,ports.replace('443,8443,10000',bad)]}, 'funnel-ports capability'))
        for suffix in ('', '?other=443', '?ports', '?ports=443&broken', '-extra?ports=443'):
            cases.append(({'CapMap':dict.fromkeys([https,funnel,ports.split('?')[0]+suffix], [])}, 'funnel-ports capability'))
        before = (self.stack/'.env').read_bytes()
        for fields, message in cases:
            with self.subTest(fields=fields, message=message):
                (self.stack/'.env').write_bytes(before)
                (self.p/'sans.json').write_text(json.dumps(OLD))
                (self.p/'docker.jsonl').unlink(missing_ok=True)
                result = self.funnel('--ts-authkey-file', str(self.key), env={'CAP_FIELDS':json.dumps(fields)}, ok=message is None)
                if message is None:
                    self.assertIn('PASS funnel '+FQDN, result.stdout)
                else:
                    self.assert_disabled(result)
                    self.assertIn(message, result.stderr)
                    if message=='https capability missing': self.assertIn('enable HTTPS certificates', result.stderr)
                    if message=='funnel capability missing': self.assertIn('add the funnel nodeAttr for tag:porchlight', result.stderr)
                    if message=='funnel-ports capability': self.assertIn('policy permits port 443', result.stderr)
                    self.assertNotIn('Traceback', result.stderr)
                    self.assertTrue((self.stack/'tailscale/authkey').exists())
                    self.assertFalse(any('server-cert' in x['argv'] for x in self.records()))

    def test_only_ts_net_names_are_published(self):
        for name in ('quiet.example.com.', 'quiet.ts.net.evil.', 'ts.net.'):
            with self.subTest(name=name):
                (self.p/'docker.jsonl').unlink(missing_ok=True)
                result = self.funnel('--ts-authkey-file', str(self.key), env={'DNS_NAME':name}, ok=False)
                self.assert_disabled(result)

    def test_serve_config_must_be_accepted_before_certificate_change(self):
        for bad in ('allow', 'target', 'proxy'):
            with self.subTest(bad=bad):
                (self.p/'docker.jsonl').unlink(missing_ok=True)
                (self.p/'serve-count').unlink(missing_ok=True)
                result = self.funnel('--ts-authkey-file', str(self.key), env={'BAD_SERVE':bad}, ok=False)
                self.assert_disabled(result)
                self.assertTrue((self.stack/'tailscale/authkey').exists())

    def test_serve_config_is_retried_with_remaining_budget(self):
        self.funnel('--ts-authkey-file', str(self.key), env={'BAD_SERVE':'allow', 'SERVE_PENDING':'1'})
        calls = [x['argv'] for x in self.records()]
        serves = [i for i,a in enumerate(calls) if a[-3:]==['serve','status','--json']]
        self.assertGreaterEqual(len(serves), 2)
        cert = next(i for i,a in enumerate(calls) if 'server-cert' in a and '--show' not in a)
        self.assertLess(serves[-1], cert)
        self.assertTrue(all(0 < int(x) <= 120 for x in (self.p/'timeouts').read_text().splitlines()))

    def test_no_key_or_node_state_refuses_before_change(self):
        before = (self.stack/'.env').read_bytes()
        result = self.funnel(ok=False)
        self.assertIn('auth key or existing node state', result.stderr)
        self.assertEqual((self.stack/'.env').read_bytes(), before)
        self.assertFalse((self.stack/'tmp').exists())
        self.assertFalse((self.stack/'tailscale').exists())
        self.assertEqual(self.records(), [])

    def test_private_state_directories_precede_compose(self):
        self.funnel('--ts-authkey-file', str(self.key))
        for record in self.records():
            if record['argv'][0]=='compose':
                self.assertEqual(record['state_dirs'], [[0o700,os.getuid()],[0o700,os.getuid()]])

    def test_foreign_owned_state_directories_are_refused(self):
        # Offline ownership simulation: no sudo/chown, and no non-fixture path touched.
        real_python = shutil.which('python3')
        shim = self.p/'bin/python3'
        shim.write_text('#!'+real_python+'\n'+'''import os,sys
if sys.argv[1:4]==['-I','-B','-']: del sys.argv[2]
code=sys.stdin.read()
if "root/'tailscale/state'" in code and 'os.getuid()' in code:
 code="import os; os.getuid=lambda: -1\\n"+code
os.execv('''+repr(real_python)+''', ['python3','-I','-c',code,*sys.argv[3:]])
''')
        shim.chmod(0o755)
        ts = self.stack/'tailscale'; (ts/'state').mkdir(parents=True)
        (ts/'state/tailscaled.state').write_text('fixture-node-state')
        result = self.funnel(ok=False)
        self.assertIn('owner', result.stderr)
        self.assertFalse((self.stack/'tmp').exists())
        self.assertEqual(self.records(), [])

    def python_shim(self, patch):
        real_python = shutil.which('python3')
        shim = self.p/'bin/python3'
        shim.write_text('#!'+real_python+'\nimport os,sys\n'
                        +'if sys.argv[1:4]==["-I","-B","-"]: del sys.argv[2]\n'
                        +'if sys.argv[1:3]!=["-I","-"]: os.execv('+repr(real_python)+', ["python3",*sys.argv[1:]])\n'
                        +'code=sys.stdin.read()\n'+patch+'\n'
                        +'os.execv('+repr(real_python)+', ["python3","-I","-c",code,*sys.argv[3:]])\n')
        shim.chmod(0o755)

    def test_signal_after_enabling_env_write_disables_before_exit(self):
        self.python_shim('if "updates=dict(zip" in code:\n'
                         ' code += "\\nif updates.get(\'FUNNEL_ENABLED\')==\'1\':\\n import signal,time; os.kill(os.getppid(),signal.SIGTERM); time.sleep(0.1)\\n"')
        result = self.funnel('--ts-authkey-file', str(self.key), ok=False)
        self.assertEqual(result.returncode, 143)
        # A signal during a redirected command can also redirect the EXIT trap's diagnostics.
        # Restoration must be verified from persisted state, independent of output availability.
        self.assert_disabled()
        self.assertTrue((self.stack/'tailscale/authkey').exists())
        self.assert_only_diagnostics()
        self.assertFalse(any('up' in x['argv'] and x['argv'][-1]=='tailscale' for x in self.records()))

    def test_cleanup_children_ignore_second_signals_and_are_bounded(self):
        result = self.funnel('--ts-authkey-file', str(self.key), env={'FAIL_CERT':'1','CLEANUP_SIGNALS':'1'}, ok=False)
        self.assert_disabled(result)
        self.assertEqual((self.p/'api-running').read_text(), '1')
        self.assertEqual([json.loads(x) for x in (self.p/'cleanup-signals').read_text().splitlines()], [[True]*3]*2)
        # Removal and API recovery each have a 30-second Docker client budget.
        self.assertEqual((self.p/'timeouts').read_text().splitlines()[-2:], ['30','30'])
        self.assert_only_diagnostics()

    def test_capability_lag_after_running_is_retried_within_grace(self):
        lagging = json.dumps({'CapMap':{'https':[], 'https://tailscale.com/cap/funnel-ports?ports=443':[]}})
        result = self.funnel('--ts-authkey-file', str(self.key),
                             env={'CAP_FIELDS_FIRST':lagging, 'PORCHLIGHT_FUNNEL_CAP_GRACE_SECS':'5'})
        self.assertIn('PASS funnel '+FQDN, result.stdout)
        self.assertGreaterEqual(int((self.p/'status-count').read_text()), 2)

    def test_capability_gap_fails_after_grace_with_specific_message(self):
        missing = json.dumps({'CapMap':{'https':[], 'https://tailscale.com/cap/funnel-ports?ports=443':[]}})
        result = self.funnel('--ts-authkey-file', str(self.key), ok=False,
                             env={'CAP_FIELDS':missing, 'PORCHLIGHT_FUNNEL_CAP_GRACE_SECS':'1'})
        self.assert_disabled(result)
        self.assertIn('add the funnel nodeAttr for tag:porchlight', result.stderr)
        self.assertGreaterEqual(int((self.p/'status-count').read_text()), 2)

    def test_server_cert_rotation_recreate_failure_reports_state_and_recovery_command(self):
        before = (self.stack/'.env').read_text()
        for recovery_fails, enabled in ((False,False), (True,False), (True,True)):
            with self.subTest(recovery_fails=recovery_fails, enabled=enabled):
                (self.stack/'.env').write_text(before + ('FUNNEL_ENABLED=1\nTS_HOSTNAME=quiet-node\nTAILSCALE_IMAGE_SHA256='+('a'*64)+'\nTS_AUTHKEY_SPEC=\n' if enabled else ''))
                (self.p/'docker.jsonl').unlink(missing_ok=True)
                result = self.run_install('server-cert', '--san-dns', 'porchlight.lan', '--san-dns', 'new.lan',
                                          env={'FAIL_RECREATE':'api', **({'FAIL_RECOVERY':'1'} if recovery_fails else {})}, ok=False)
                self.assertIn('Certificate rotated; API recreate failed', result.stderr)
                self.assertIn('API not running' if recovery_fails else 'API recovery succeeded', result.stderr)
                self.assertNotIn('PASS server-cert', result.stdout)
                self.assertEqual((self.p/'api-running').read_text(), '0' if recovery_fails else '1')
                self.assertEqual(json.loads((self.p/'sans.json').read_text())['dns'], ['porchlight.lan','new.lan'])
                command = next(x.removeprefix('Recovery command: ') for x in result.stderr.splitlines() if x.startswith('Recovery command: '))
                tokens = shlex.split(command)
                for value in (str(self.stack), str(self.p/'run'), str(self.stack/'.env'), str(self.release/'compose.yml'), 'porchlight-fixture'):
                    self.assertTrue(any(value in token for token in tokens), value)
                if enabled:
                    self.assertIn(str(self.release/'compose.funnel.yml'), tokens)
                    self.assertIn('--profile', tokens)
                    self.assertIn('funnel', tokens)
                else:
                    self.assertNotIn('compose.funnel.yml', command)
                recovery = subprocess.run(['bash','-c',command], env=self.env, stdin=subprocess.DEVNULL,
                                          capture_output=True, text=True, timeout=30, start_new_session=True)
                self.assertEqual(recovery.returncode, 0, recovery.stderr[:500])
                self.assertEqual((self.p/'api-running').read_text(), '1')
                self.assert_only_diagnostics()

    def test_state_directory_fd_owner_recheck_is_clean(self):
        self.python_shim('if "os.O_DIRECTORY" in code and "os.fstat(fd).st_uid" in code:\n'
                         ' code="import os; os.getuid=lambda: -1\\n"+code')
        before = (self.stack/'.env').read_bytes()
        result = self.funnel('--ts-authkey-file', str(self.key), ok=False)
        self.assertIn('Funnel state directories must be owner-held real directories', result.stderr)
        self.assertNotIn('Traceback', result.stderr)
        self.assertEqual(self.records(), [])
        self.assertEqual((self.stack/'.env').read_bytes(), before)

    def test_symlink_stack_root_refused_before_staging(self):
        alias = self.p/'stack-alias'; alias.symlink_to(self.stack, target_is_directory=True)
        result = self.funnel('--ts-authkey-file', str(self.key), stack=alias, ok=False)
        self.assertIn('no symlinks', result.stderr)
        self.assertNotIn('Traceback', result.stderr)
        self.assertFalse((self.stack/'tmp').exists())
        self.assertEqual(self.records(), [])

    def test_rerun_does_not_rotate_or_restart_api_or_sidecar(self):
        self.funnel('--ts-authkey-file', str(self.key))
        (self.p/'docker.jsonl').unlink()
        self.funnel()
        calls = [x['argv'] for x in self.records()]
        self.assertFalse(any('server-cert' in a and '--show' not in a for a in calls))
        self.assertFalse(any('stop' in a and a[-1]=='api' for a in calls))
        self.assertFalse(any('up' in a and a[-1]=='api' for a in calls))
        self.assertFalse(any('--force-recreate' in a and a[-1]=='tailscale' for a in calls))

    def test_server_cert_public_results_and_failure_reason(self):
        result = self.run_install('server-cert', '--san-dns', 'porchlight.lan')
        self.assertIn('previous SANs:', result.stdout)
        self.assertIn('not_after:', result.stdout)
        result = self.run_install('server-cert', '--san-dns', 'porchlight.lan', env={'FAIL_CERT':'1'}, ok=False)
        self.assertIn('invalid DNS SAN: fixture reason', result.stderr)

    def server_release(self, version='0.1.1'):
        source = self.p/('server-release-'+version); source.mkdir()
        shutil.copyfile(self.installer, source/'install.sh')
        for name in ('compose.yml', 'compose.funnel.yml'):
            shutil.copyfile(ROOT/'deploy'/name, source/name)
        for service in ('api', 'backup'):
            (source/f'porchlight-{service}-{version}.tar').write_text('fixture image')
        subprocess.run(['python3','-I',str(ROOT/'deploy/lib/operations.py'),'manifest',str(source),version], check=True)
        (source/'manifest.json.minisig').write_text('fake-signature')
        return source

    def restore_bundle(self, enabled):
        # Restore now validates and persists explicit target endpoints, rather than ignoring them.
        self.env.pop('PORCHLIGHT_ENDPOINTS', None)
        import tarfile
        tree = self.p/'bundle-tree'; tree.mkdir()
        for path in ('db.dump','pki/ca.crt','pki/ca.key','secrets/idempotency.key','backup-recipient.txt'):
            target=tree/path; target.parent.mkdir(parents=True,exist_ok=True); target.write_text('fixture')
        (tree/'blobs').mkdir()
        (tree/'.env').write_text('PORCHLIGHT_VERSION=0.1.1\nFUNNEL_ENABLED='+str(enabled)+'\n'
                                +'TS_HOSTNAME=quiet-node\nTAILSCALE_IMAGE_SHA256='+'a'*64+'\n'
                                +'TS_AUTHKEY_SPEC=file:/config/authkey\n'
                                +'PORCHLIGHT_ENDPOINTS=https://porchlight.lan:8443,https://'+FQDN+'\n')
        subprocess.run(['python3','-I',str(ROOT/'deploy/lib/operations.py'),'manifest',str(tree),'0.1.1'], check=True)
        bundle = self.p/'restore.tar.age'
        with tarfile.open(bundle,'w') as archive:
            for child in tree.iterdir(): archive.add(child,arcname=child.name)
        identity = self.p/'identity'; identity.write_text('fixture identity')
        age = self.p/'bin/age'
        age.write_text('#!/usr/bin/env python3\nimport shutil,sys\nshutil.copyfile(sys.argv[-1],sys.argv[sys.argv.index("-o")+1])\n')
        age.chmod(0o755)
        return bundle, identity

    def test_restore_always_starts_lan_only_and_prints_reenable(self):
        self.funnel('--ts-authkey-file', str(self.key))
        source = self.server_release()
        bundle, identity = self.restore_bundle(1)
        (self.p/'docker.jsonl').unlink()
        result = self.run_install('restore', str(bundle), '--identity', str(identity), '--source', str(source), '--force')
        self.assertIn('LAN-only', result.stdout)
        self.assertLess(result.stdout.index('PASS restore'), result.stdout.index('Re-enable explicitly'))
        import shlex
        retry = shlex.split(next(line for line in result.stdout.splitlines() if line.startswith('bash ')))
        self.assertEqual(retry[:3], ['bash', str(self.stack/'release-current/install.sh'), 'funnel'])
        self.assertIn('--no-authkey', retry)
        values=self.env_values()
        self.assertEqual(values['FUNNEL_ENABLED'], '0')
        self.assertEqual(values['TS_AUTHKEY_SPEC'], '')
        self.assertEqual(values['TS_HOSTNAME'], 'quiet-node')
        self.assertEqual(values['TAILSCALE_IMAGE_SHA256'], 'a'*64)
        self.assertEqual(values['PORCHLIGHT_ENDPOINTS'], 'https://porchlight.lan:8443,https://'+FQDN)
        calls=[x['argv'] for x in self.records() if '--env-file' in x['argv']]
        self.assertIn('--profile', next(a for a in calls if a[-1]=='down'))
        self.assertFalse(any('up' in a and a[-1]=='tailscale' for a in calls))
        self.assertTrue(any('up' in a and a[-1]=='api' for a in calls))
        self.assertTrue(all('--profile' not in a for a in calls if a[-1]!='down'))
        self.funnel('--no-authkey')
        self.assertEqual(self.env_values()['FUNNEL_ENABLED'], '1')

    def test_restore_new_root_never_creates_tailscale_and_requests_key(self):
        source = self.server_release()
        bundle, identity = self.restore_bundle(1)
        fresh = self.p/'fresh-root'
        result = self.run_install('restore', str(bundle), '--identity', str(identity), '--source', str(source), stack=fresh)
        self.assertIn('LAN-only', result.stdout)
        self.assertIn('--ts-authkey-file', result.stdout)
        self.assertIn('new', result.stdout)
        self.assertFalse((fresh/'tailscale').exists())
        self.assertFalse(any('--profile' in x['argv'] for x in self.records()))

    def test_restore_disabled_bundle_has_no_reenable_even_on_enabled_stack(self):
        self.funnel('--ts-authkey-file', str(self.key))
        source = self.server_release()
        bundle, identity = self.restore_bundle(0)
        result=self.run_install('restore', str(bundle), '--identity', str(identity), '--source', str(source), '--force', '--no-start')
        self.assertNotIn('Re-enable', result.stdout)
        self.assertNotIn('--ts-authkey-file', result.stdout)
        self.assertEqual(self.env_values()['FUNNEL_ENABLED'], '0')
        self.assertEqual(self.env_values()['TS_AUTHKEY_SPEC'], '')

    def test_restore_failure_does_not_print_reenable(self):
        source = self.server_release()
        bundle, identity = self.restore_bundle(1)
        result=self.run_install('restore', str(bundle), '--identity', str(identity), '--source', str(source),
                                '--force', env={'FAIL_RESTORE':'1'}, ok=False)
        self.assertNotIn('PASS restore', result.stdout)
        self.assertNotIn('Re-enable', result.stdout)
        self.assertNotIn('--ts-authkey-file', result.stdout)
        self.assertEqual(self.env_values()['FUNNEL_ENABLED'], '0')
        self.assertEqual(self.env_values()['TS_AUTHKEY_SPEC'], '')

    def test_upgrade_starts_sidecar_iff_enabled(self):
        source = self.server_release()
        self.run_install('upgrade', '--source', str(source))
        calls=[x['argv'] for x in self.records()]
        self.assertTrue(any('up' in a and a[-1]=='api' for a in calls))
        self.assertFalse(any('up' in a and a[-1]=='tailscale' for a in calls))
        self.assertTrue(all('--profile' not in a for a in calls))
        self.funnel('--ts-authkey-file', str(self.key))
        (self.p/'docker.jsonl').unlink()
        source = self.server_release('0.1.2')
        self.run_install('upgrade', '--source', str(source))
        calls=[x['argv'] for x in self.records() if '--env-file' in x['argv']]
        self.assertTrue(any('up' in a and '--wait' in a and a[-1]=='tailscale' for a in calls))
        self.assertTrue(all('--profile' in a for a in calls))

    def test_validation_precedes_all_changes(self):
        before = (self.stack/'.env').read_bytes()
        for hostname, image in [('Bad.Name',IMAGE),('a'*64,IMAGE),('ok','tailscale/tailscale:latest'),
                                ('ok','tailscale/tailscale@sha256:'+'A'*64),('ok',IMAGE+'x')]:
            self.run_install('funnel', '--ts-hostname', hostname, '--tailscale-image', image, ok=False)
        self.assertEqual((self.stack/'.env').read_bytes(), before)
        self.assertFalse((self.stack/'tmp').exists())
        self.assertFalse((self.stack/'tailscale').exists())
        self.assertEqual(self.records(), [])

    def test_existing_stack_required_before_changes(self):
        missing = self.p/'missing-stack'
        self.funnel(stack=missing, ok=False)
        self.assertFalse(missing.exists())
        (self.stack/'release-current').unlink()
        self.funnel(ok=False)
        self.assertFalse((self.stack/'tailscale').exists())
        self.assertEqual(self.records(), [])

    def test_authkey_private_regular_file(self):
        self.key.chmod(0o644)
        self.funnel('--ts-authkey-file', str(self.key), ok=False)
        self.key.chmod(0o600)
        link = self.p/'key-link'; link.symlink_to(self.key)
        self.funnel('--ts-authkey-file', str(link), ok=False)
        self.assertFalse((self.stack/'tailscale').exists())
        self.assertFalse((self.stack/'tmp').exists())
        self.assertEqual(self.records(), [])

    def test_nonregular_authkey_target_rejected_before_change(self):
        ts = self.stack/'tailscale'; ts.mkdir()
        target = ts/'authkey'; target.mkdir()
        before = (self.stack/'.env').read_bytes()
        self.funnel('--ts-authkey-file', str(self.key), ok=False)
        self.assertEqual((self.stack/'.env').read_bytes(), before)
        self.assertFalse((self.stack/'tmp').exists())
        self.assertEqual(list(ts.iterdir()), [target])
        self.assertEqual(self.records(), [])

    def test_login_timeout_cannot_be_ignored_by_docker(self):
        result = self.funnel('--ts-authkey-file', str(self.key), env={'TIMEOUT_STATUS':'124'}, ok=False)
        self.assertIn('within 120 s', result.stderr)
        self.assertEqual(set((self.p/'timeout-signals').read_text().splitlines()), {'True'})

    def test_failed_start_retains_private_key_for_retry(self):
        self.funnel('--ts-authkey-file', str(self.key), env={'FAIL_START':'1'}, ok=False)
        key = self.stack/'tailscale/authkey'
        self.assertEqual(stat.S_IMODE(key.stat().st_mode), 0o600)
        self.assertEqual(key.read_bytes(), self.key.read_bytes())
        self.assertEqual(self.env_values()['PORCHLIGHT_ENDPOINTS'], 'https://porchlight.lan:8443')
        self.assertTrue(all('server-cert' not in x['argv'] for x in self.records()))
        self.funnel()
        self.assertFalse(key.exists())

    def test_login_timeout_is_bounded_and_keeps_key(self):
        result = self.funnel('--ts-authkey-file', str(self.key), env={'TIMEOUT_STATUS':'137'}, ok=False)
        self.assertIn('within 120 s', result.stderr)
        self.assertTrue((self.stack/'tailscale/authkey').is_file())
        self.assertEqual(self.env_values()['PORCHLIGHT_ENDPOINTS'], 'https://porchlight.lan:8443')
        self.assertTrue(all('server-cert' not in x['argv'] for x in self.records()))
        self.assertTrue(all(0 < int(x) <= 120 for x in (self.p/'timeouts').read_text().splitlines()))

    def test_funnel_post_login_failures_disable_and_report_unchanged(self):
        for failure in ({'FAIL_CERT':'1'}, {'FAIL_SHOW':'1'}, {'FAIL_RECREATE':'tailscale'}):
            with self.subTest(failure=failure):
                (self.p/'docker.jsonl').unlink(missing_ok=True)
                result = self.funnel('--ts-authkey-file', str(self.key), env=failure, ok=False)
                self.assertFalse((self.stack/'tailscale/authkey').exists())
                if 'FAIL_CERT' in failure:
                    self.assertEqual((self.p/'api-running').read_text(), '1')
                    self.assertIn('invalid DNS SAN: fixture reason', result.stderr)
                if 'FAIL_SHOW' in failure:
                    self.assertIn('cannot read certificate: fixture reason', result.stderr)
                self.assert_disabled(result)
                self.assertIn('Certificate/endpoints unchanged by this activation', result.stderr)
                self.assertIn('--no-authkey', result.stderr)

    def test_late_recreate_failures_report_rotated_certificate_and_endpoints(self):
        for service in ('api','backup'):
            with self.subTest(service=service):
                (self.stack/'.env').write_text('PORCHLIGHT_ENDPOINTS=https://porchlight.lan:8443\n')
                (self.p/'sans.json').write_text(json.dumps(OLD))
                (self.p/'docker.jsonl').unlink(missing_ok=True)
                result = self.funnel('--ts-authkey-file', str(self.key), env={'FAIL_RECREATE':service}, ok=False)
                self.assert_disabled(result, changed=True)
                self.assertEqual((self.p/'api-running').read_text(), '1')
                self.assertIn(FQDN, json.loads((self.p/'sans.json').read_text())['dns'])
                self.assertEqual(self.env_values()['PORCHLIGHT_ENDPOINTS'], 'https://porchlight.lan:8443,https://'+FQDN)
                self.assertIn('Certificate/endpoints already include '+FQDN, result.stderr)
                self.assertIn('--no-authkey to complete', result.stderr)

    def test_endpoint_write_failure_reports_certificate_only_and_recovers(self):
        self.python_shim('if "updates=dict(zip" in code and sys.argv[4:5]==["PORCHLIGHT_ENDPOINTS"]: sys.exit(1)')
        result = self.funnel('--ts-authkey-file', str(self.key), ok=False)
        self.assert_disabled(result, changed=True)
        self.assertEqual((self.p/'api-running').read_text(), '1')
        self.assertIn(FQDN, json.loads((self.p/'sans.json').read_text())['dns'])
        self.assertEqual(self.env_values()['PORCHLIGHT_ENDPOINTS'], 'https://porchlight.lan:8443')
        self.assertIn('Certificate already includes '+FQDN+'; endpoints not updated', result.stderr)
        self.assertIn('--no-authkey to complete', result.stderr)

    def test_bad_san_metadata_does_not_stop_api(self):
        (self.p/'sans.json').write_text(json.dumps({'dns':['bad\\nname'],'ips':[]}))
        self.funnel('--ts-authkey-file', str(self.key), ok=False)
        self.assertEqual(self.env_values()['PORCHLIGHT_ENDPOINTS'], 'https://porchlight.lan:8443')
        self.assertTrue(all('stop' not in x['argv'] for x in self.records()))

    def test_state_symlinks_rejected_before_change(self):
        outside = self.p/'other-state'; outside.mkdir()
        (self.stack/'tailscale').symlink_to(outside)
        self.funnel('--ts-authkey-file', str(self.key), ok=False)
        self.assertEqual(list(outside.iterdir()), [])
        self.assertFalse((self.stack/'tmp').exists())
        self.assertEqual(self.records(), [])

    def test_bad_fqdn_does_not_rotate_or_publish(self):
        self.funnel('--ts-authkey-file', str(self.key), env={'DNS_NAME':'bad name.ts.net.'}, ok=False)
        self.assertEqual(self.env_values()['PORCHLIGHT_ENDPOINTS'], 'https://porchlight.lan:8443')
        self.assertTrue(all('server-cert' not in x['argv'] for x in self.records()))

    def test_server_cert_stop_run_start_and_funnel_selection(self):
        self.run_install('server-cert', '--san-dns', 'porchlight.lan', '--san-ip', '192.168.77.50')
        calls = [x['argv'] for x in self.records() if '--env-file' in x['argv']]
        self.assertEqual(len(calls), 3)
        self.assertEqual(calls[0][-2:], ['stop','api'])
        self.assertEqual(calls[1][-10:], ['run','--rm','--no-deps','-T','api','server-cert','--san-dns','porchlight.lan','--san-ip','192.168.77.50'])
        self.assertIn('up', calls[2]); self.assertEqual(calls[2][-1], 'api')
        self.assertTrue(all('--profile' not in a for a in calls))
        with (self.stack/'.env').open('a') as f: f.write('FUNNEL_ENABLED=1\n')
        self.run_install('backup')
        self.assertIn('--profile', self.records()[-1]['argv'])
        with (self.stack/'.env').open('a') as f: f.write('FUNNEL_ENABLED=0\n')
        self.run_install('backup')
        self.assertNotIn('--profile', self.records()[-1]['argv'])

    def test_server_cert_requires_sans_and_recovers_after_failure(self):
        self.run_install('server-cert', ok=False)
        self.assertEqual(self.records(), [])
        self.run_install('server-cert', '--san-dns', 'porchlight.lan', env={'FAIL_CERT':'1'}, ok=False)
        self.assertEqual(self.records()[-1]['argv'][-1], 'api')
        self.assertIn('up', self.records()[-1]['argv'])

    def test_server_funnel_refusal_points_to_mode(self):
        result = self.run_install('server', '--funnel', ok=False)
        self.assertIn('install.sh funnel', result.stderr)
        self.assertFalse((self.stack/'tmp').exists())

    def test_niri_different_path_warns_without_duplicate(self):
        source = self.p/'desktop-release'; source.mkdir()
        shutil.copyfile(self.installer, source/'install.sh')
        shutil.copytree(ROOT/'deploy/lib', source/'lib', ignore=shutil.ignore_patterns('__pycache__'))
        shutil.copyfile(ROOT/'design/icon/porchlight-icon.svg', source/'porchlight.svg')
        image = self.p/'image/porchlight/bin'; image.mkdir(parents=True)
        launcher = image/'porchlight'; launcher.write_text('#!/bin/sh\nexit 0\n'); launcher.chmod(0o755)
        subprocess.run(['tar','-czf',str(source/'porchlight-desktop-0.1.0-linux-x86_64.tar.gz'),'-C',str(self.p/'image'),'porchlight'],check=True)
        subprocess.run(['python3','-I',str(ROOT/'deploy/lib/operations.py'),'manifest',str(source),'0.1.0'],check=True)
        (source/'manifest.json.minisig').write_text('fake-signature')
        config = self.p/'config/niri/config.kdl'; config.parent.mkdir(parents=True)
        original = 'spawn-at-startup "/old/install/bin/porchlight" "--tray"\n'
        config.write_text(original)
        result = self.run_install('desktop','--source',str(source),'--niri')
        self.assertIn('niri spawn-at-startup names a different bin/porchlight path', result.stderr)
        self.assertEqual(config.read_text(), original)
        current = self.p/'desktop/porchlight/current/app/bin/porchlight'
        config.write_text('spawn-at-startup '+json.dumps(str(current))+' "--minimized"\n')
        result = self.run_install('desktop','--source',str(source),'--niri')
        self.assertNotIn('different bin/porchlight path', result.stderr)
        self.assertEqual(len(config.read_text().splitlines()), 1)

    def test_compose_contract(self):
        text = (ROOT/'deploy/compose.funnel.yml').read_text()
        for value in ['TS_USERSPACE: "true"','TS_AUTH_ONCE: "true"','TS_STATE_DIR: /var/lib/tailscale',
                      'TS_SERVE_CONFIG: /config/serve.json','TS_HOSTNAME: "${TS_HOSTNAME:?',
                      'TS_EXTRA_ARGS: "--advertise-tags=tag:porchlight"','TS_AUTHKEY: "${TS_AUTHKEY_SPEC:-}"',
                      'TS_SOCKET: /var/run/tailscale/tailscaled.sock',
                      'security_opt: ["no-new-privileges:true"]',
                      'target: /var/lib/tailscale', 'target: /config', 'read_only: true',
                      'bind: {create_host_path: false}',
                      'tailscale/tailscale@sha256:${TAILSCALE_IMAGE_SHA256:?', '${FUNNEL_IP:-172.29.251.10}']:
            self.assertIn(value, text)
        self.assertNotIn('ports:', text)

unittest.main(verbosity=1)
PY
if [[ ${1:-} == --sandbox ]]; then
  printf 'SKIP real Compose config (sandbox)\n'
elif command -v docker >/dev/null && docker compose version >/dev/null 2>&1; then
  # No daemon/network needed: dummy .env only, never render a production configuration.
  dummy=$(mktemp "$root/var/tests/funnel-env.XXXXXX")
  trap 'rm -f "$dummy"' EXIT
  printf '%s\n' 'POSTGRES_PASSWORD=fixture' 'PORCHLIGHT_MIGRATOR_PASSWORD=fixture' \
    'PORCHLIGHT_APP_PASSWORD=fixture' 'PORCHLIGHT_AGENT_PASSWORD=fixture' 'PORCHLIGHT_BACKUP_PASSWORD=fixture' \
    'PORCHLIGHT_VERSION=0.1.0' 'ENROLL_CIDRS=192.168.77.0/24' 'TS_HOSTNAME=quiet-node' \
    'TAILSCALE_IMAGE_SHA256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' \
    'TS_AUTHKEY_SPEC=' > "$dummy"
  env -u POSTGRES_PASSWORD -u PORCHLIGHT_MIGRATOR_PASSWORD -u PORCHLIGHT_APP_PASSWORD \
    -u PORCHLIGHT_AGENT_PASSWORD -u PORCHLIGHT_BACKUP_PASSWORD -u PORCHLIGHT_VERSION \
    -u ENROLL_CIDRS -u TS_HOSTNAME -u TAILSCALE_IMAGE_SHA256 -u TS_AUTHKEY_SPEC \
    PORCHLIGHT_ROOT="$root/var/compose-funnel-test" PORCHLIGHT_RUN_DIR="$root/var/run-funnel-test" \
    docker compose --env-file "$dummy" -f "$root/deploy/compose.yml" -f "$root/deploy/compose.funnel.yml" --profile funnel config -q
  printf 'PASS real Compose Funnel config\n'
else
  printf 'SKIP real Compose config (Docker Compose unavailable)\n'
fi
