#!/usr/bin/env python3
"""Exercise generated trusted installers against a loopback release mirror, never Docker."""
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import stat
import subprocess
import tarfile
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[2]
OLD, NEW = '0.1.6', '0.1.7'


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


OPS = module('ops', ROOT / 'deploy/lib/operations.py')
BUILDER = module('builder', ROOT / 'scripts/build-installer.py')
DOCKER = r'''#!/usr/bin/env -S python3 -I -B
import json,os,pathlib,sys
p=pathlib.Path(os.environ['FIXTURE']);root=pathlib.Path(os.environ['PORCHLIGHT_ROOT']);a=sys.argv[1:]
e=dict(x.split('=',1) for x in (root/'.env').read_text().splitlines() if '=' in x) if (root/'.env').exists() else {}
v=e.get('PORCHLIGHT_VERSION','')
record={'argv':a,'version':v,'current':os.readlink(root/'release-current') if (root/'release-current').is_symlink() else None}
if 'pg_restore' in a:
 record['dump']=sys.stdin.read()
if 'pg_dump' in a:
 dump=root/'rollback'/('pre-upgrade-'+v+'.dump')
 record['dump_mode']=dump.stat().st_mode & 0o777
 record['dir_mode']=dump.parent.stat().st_mode & 0o777
with (p/'docker.jsonl').open('a') as f: f.write(json.dumps(record)+'\n')
if a in (['info'],['compose','version']) or a[0]=='ps': sys.exit(0)
if a[0]=='load': sys.exit(1 if os.environ.get('FAIL_AT')=='load' else 0)
assert a[0]=='compose'
assert a[a.index('-f')+1]==str(root/record['current']/'compose.yml')
assert not any(k in os.environ for k in ('PORCHLIGHT_RELEASE_REPO','PORCHLIGHT_RELEASE_PUBLIC_KEY','PORCHLIGHT_UPGRADE_COMMAND','PORCHLIGHT_VERSION'))
if a[-2:]==['backup','now']:
 name='porchlight-20261010T120000000000Z.tar.age'
 (root/'backups').mkdir(exist_ok=True);(root/'backups'/name).write_text('encrypted fixture')
 print('PASS backup '+name)
if 'pg_dump' in a: print('plain rollback fixture')
if 'pg_restore' in a and os.environ.get('FAIL_ROLLBACK'): sys.exit(1)
if v=='0.1.7':
 fail=os.environ.get('FAIL_AT')
 if fail=='migrate' and '--exit-code-from' in a: sys.exit(1)
 if fail=='health' and a[-1]=='healthcheck': sys.exit(1)
 if fail=='start' and 'up' in a and a[-1]=='api': sys.exit(1)
'''
MINISIGN = r'''#!/usr/bin/env -S python3 -I -B
import hashlib,os,pathlib,sys
a=sys.argv[1:]
assert a[a.index('-P')+1]=='FIXTURE-PINNED-PUBLIC-KEY'
m=pathlib.Path(a[a.index('-Vm')+1]);s=pathlib.Path(a[a.index('-x')+1])
sys.exit(0 if s.read_text()==hashlib.sha256(m.read_bytes()).hexdigest() else 1)
'''


class UpgradeTests(unittest.TestCase):
    def setUp(self):
        (ROOT / 'var/tests').mkdir(parents=True, exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(prefix='github-upgrade.', dir=ROOT / 'var/tests')
        self.addCleanup(self.tmp.cleanup)
        self.p = Path(self.tmp.name)
        self.stack = self.p / 'stack with spaces'
        self.run_dir = self.p / 'run with spaces'
        self.stack.mkdir()
        self.release = self.make_release(NEW)
        old = self.make_release(OLD)
        (self.stack / 'releases').mkdir()
        shutil.copytree(old, self.stack / 'releases' / OLD)
        (self.stack / 'release-current').symlink_to('releases/' + OLD)
        (self.stack / '.env').write_text('PORCHLIGHT_VERSION=' + OLD + '\nPORCHLIGHT_RELEASE_REPO=example-owner/porchlight\nPOSTGRES_PASSWORD=PRIVATE-FIXTURE-NEVER-PRINT\n')
        (self.stack / '.env').chmod(0o600)
        self.before = (self.stack / '.env').read_bytes()
        self.installer = old / 'install.sh'
        binpath = self.p / 'bin'
        binpath.mkdir()
        for name, content in [('docker', DOCKER), ('minisign', MINISIGN)]:
            (binpath / name).write_text(content)
            (binpath / name).chmod(0o755)
        self.payloads = {}
        self.requests = []
        self.behavior = ''
        self.publish(NEW)
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                fixture.requests.append(self.path)
                if fixture.behavior == 'redirect':
                    self.send_response(302)
                    self.send_header('Location', 'https://example.invalid/escape')
                    self.end_headers()
                    return
                if fixture.behavior == 'local-redirect' and self.path.startswith('/latest/'):
                    self.send_response(302)
                    self.send_header('Location', self.path.replace('/latest/download/', '/download/v' + NEW + '/'))
                    self.end_headers()
                    return
                if fixture.behavior in ('oversize', 'oversize-tar') and (fixture.behavior == 'oversize' or self.path.endswith('.tar')):
                    self.send_response(200)
                    self.send_header('Content-Length', str(2 * 1024**3 + 1))
                    self.end_headers()
                    return
                data = fixture.payloads.get(self.path)
                if fixture.behavior == 'oversize-stream' and self.path.endswith('/manifest.json'):
                    data = b' ' * (1024**2 + 1)
                if data is None:
                    self.send_error(404)
                    return
                self.send_response(200)
                if fixture.behavior != 'oversize-stream':
                    self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.addCleanup(self.server.server_close)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.stop_server)
        self.env = dict(os.environ, PATH=str(binpath) + ':/usr/bin:/bin', FIXTURE=str(self.p),
                        HOME=str(self.p / 'home'), PORCHLIGHT_INSTALL_TESTING='1',
                        PORCHLIGHT_RELEASE_BASE_URL=f'http://127.0.0.1:{self.server.server_port}',
                        PORCHLIGHT_RELEASE_REPO='example-owner/ambient-repo', PORCHLIGHT_VERSION='9.9.9')

    def stop_server(self):
        self.server.shutdown()
        self.thread.join()

    def make_release(self, version, repo='example-owner/porchlight'):
        source = self.p / ('dist-' + version)
        source.mkdir()
        installer = BUILDER.build(ROOT, 'FIXTURE-PINNED-PUBLIC-KEY')
        # Signed fixture instrumentation records the argv of the real exec handoff.
        recorder = '''python3 -I -B -c 'import json,os,pathlib,sys;p=pathlib.Path(os.environ["FIXTURE"])/"handoff.jsonl";p.open("a").write(json.dumps(sys.argv[1:])+"\\n")' "$0" "$@"\n'''
        (source / 'install.sh').write_text('#!/usr/bin/env bash\n' + recorder + installer)
        shutil.copytree(ROOT / 'deploy/lib', source / 'lib', ignore=shutil.ignore_patterns('__pycache__', '*.pyc'))
        for name in ('compose.yml', 'compose.funnel.yml'):
            shutil.copyfile(ROOT / 'deploy' / name, source / name)
        (source / 'postgres/initdb').mkdir(parents=True)
        (source / 'postgres/initdb/10-porchlight.sh').write_text('fixture')
        archive = f'porchlight-desktop-{version}-linux-x86_64.tar.gz'
        for name in (archive, 'porchlight.svg', f'porchlight-{version}.apk', f'porchlight-api-{version}.tar', f'porchlight-backup-{version}.tar'):
            (source / name).write_text('fixture')
        desktop = sorted(['install.sh', 'porchlight.svg', archive, 'release.json'] +
                         [p.relative_to(source).as_posix() for p in (source / 'lib').rglob('*') if p.is_file()])
        record = {'format_version': 1, 'version': version, 'commit': None, 'dirty': True, 'source': {'github': repo},
                  'android': {'apk': f'porchlight-{version}.apk', 'package': 'app.porchlight',
                              'version_code': OPS.version_code(version), 'signer_sha256': ['a' * 64]},
                  'desktop': {'archive': archive, 'files': desktop}}
        (source / 'release.json').write_text(json.dumps(record, indent=1, sort_keys=True) + '\n')
        self.sign(source, version)
        return source

    def sign(self, source, version):
        OPS.manifest(source, version)
        (source / 'manifest.json.minisig').write_text(hashlib.sha256((source / 'manifest.json').read_bytes()).hexdigest())

    def publish(self, version, tar_source=None):
        source = self.p / ('dist-' + version)
        archive = self.p / 'dist.tar'
        with tarfile.open(archive, 'w') as t:
            # Like publish-release.sh: regular files only, so extraction creates the directories itself.
            for p in sorted(x for x in (tar_source or source).rglob('*') if x.is_file()):
                t.add(p, arcname=p.relative_to(tar_source or source).as_posix(), recursive=False)
        for name, data in [('manifest.json', (source / 'manifest.json').read_bytes()),
                           ('manifest.json.minisig', (source / 'manifest.json.minisig').read_bytes()),
                           (f'porchlight-{version}-dist.tar', archive.read_bytes())]:
            self.payloads['/download/v' + version + '/' + name] = data
            self.payloads['/latest/download/' + name] = data

    def run_install(self, *args, expected=0, env=None, mode='upgrade', stack=None, installer=None):
        result = subprocess.run(['bash', str(installer or self.installer), mode, *args, '--root', str(stack or self.stack),
                                 '--run-dir', str(self.run_dir), '--project', 'porchlight-fixture'],
                                env=dict(self.env, **(env or {})), capture_output=True, text=True, timeout=30)
        self.assertNotIn('PRIVATE-FIXTURE-NEVER-PRINT', result.stdout + result.stderr)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return result

    def records(self):
        path = self.p / 'docker.jsonl'
        return [json.loads(x) for x in path.read_text().splitlines()] if path.exists() else []

    def unchanged(self):
        self.assertEqual((self.stack / '.env').read_bytes(), self.before)
        self.assertEqual(os.readlink(self.stack / 'release-current'), 'releases/' + OLD)
        self.assertEqual(self.records(), [])

    def test_bad_signature(self):
        self.payloads['/latest/download/manifest.json.minisig'] = b'rejected'
        result = self.run_install('--release', 'latest', expected=1)
        self.assertIn('release signature rejected', result.stderr)
        self.unchanged()
        self.assertFalse(any(p.endswith('.tar') for p in self.requests))

    def test_downgrade_and_source_downgrade(self):
        self.make_release('0.1.5')
        self.publish('0.1.5')
        for args in [('--release', 'latest'), ('--source', str(self.p / 'dist-0.1.5'))]:
            result = self.run_install(*args, expected=2)
            self.assertIn('refusing downgrade', result.stderr)
            self.unchanged()

    def test_same_version_no_changes(self):
        self.publish(OLD)
        before = (self.stack / '.env').stat().st_mtime_ns
        for args in [('--release', 'latest'), ('--source', str(self.p / ('dist-' + OLD)))]:
            result = self.run_install(*args)
            self.assertIn('PASS already at ' + OLD, result.stdout)
            self.unchanged()
            self.assertEqual((self.stack / '.env').stat().st_mtime_ns, before)
        self.assertFalse(any(p.endswith('.tar') for p in self.requests))

    def test_explicit_mismatched_version(self):
        for name in ('manifest.json', 'manifest.json.minisig'):
            self.payloads['/download/v0.1.8/' + name] = self.payloads['/latest/download/' + name]
        result = self.run_install('--release', '0.1.8', expected=2)
        self.assertIn('version mismatch', result.stderr)
        self.unchanged()

    def test_tampered_tar_member(self):
        (self.release / 'porchlight.svg').write_text('tampered')
        self.publish(NEW)
        self.run_install('--release', NEW, expected=1)
        self.unchanged()
        incoming = next((self.stack / 'incoming').glob('install.*'))
        self.assertEqual(stat.S_IMODE(incoming.stat().st_mode), 0o700)
        for name in ('manifest.json', 'manifest.json.minisig', 'dist.tar'):
            self.assertEqual(stat.S_IMODE((incoming / name).stat().st_mode), 0o600)

    def test_manifest_swapped_inside_tar(self):
        other = self.p / 'swapped'
        shutil.copytree(self.release, other)
        (other / 'porchlight.svg').write_text('different signed inventory')
        self.sign(other, NEW)
        self.publish(NEW, other)
        result = self.run_install('--release', NEW, expected=1)
        self.assertIn('manifest differs', result.stderr)
        self.unchanged()

    def test_network_policy_and_caps(self):
        for behavior in ('redirect', 'oversize', 'oversize-stream', 'oversize-tar'):
            with self.subTest(behavior=behavior):
                self.behavior = behavior
                self.run_install('--release', 'latest', expected=1)
                self.unchanged()
        self.behavior = ''
        self.requests.clear()
        self.run_install('--release', 'latest', expected=1, env={'PORCHLIGHT_INSTALL_TESTING': '0'})
        self.assertEqual(self.requests, [])
        self.unchanged()

    def test_repo_validation_and_admin_override(self):
        (self.stack / '.env').write_bytes(self.before.replace(b'example-owner/porchlight', b''))
        self.before = (self.stack / '.env').read_bytes()
        result = self.run_install('--release', 'latest', expected=2)
        self.assertIn('set PORCHLIGHT_RELEASE_REPO', result.stderr)
        self.run_install('--release', 'latest', '--repo', '../bad', expected=2)
        self.assertEqual(self.requests, [])
        self.unchanged()
        self.run_install('--release', 'latest', '--repo', 'example-owner/porchlight')
        self.assertIn('PORCHLIGHT_RELEASE_REPO=\n', (self.stack / '.env').read_text())

    def test_success_handoff_wrapper_and_cleanup(self):
        self.behavior = 'local-redirect'
        result = self.run_install('--release', 'latest')
        self.assertIn('PASS upgrade ' + NEW, result.stdout)
        self.assertEqual(os.readlink(self.stack / 'release-current'), 'releases/' + NEW)
        rows = [json.loads(x) for x in (self.p / 'handoff.jsonl').read_text().splitlines()]
        extracted = Path(rows[1][0]).parent
        self.assertEqual(rows[1], [str(extracted / 'install.sh'), 'upgrade', '--source', str(extracted),
                                  '--root', str(self.stack), '--run-dir', str(self.run_dir), '--project', 'porchlight-fixture'])
        self.assertEqual(extracted.parent.parent, self.stack / 'incoming')
        self.assertFalse(extracted.parent.exists())
        self.assertEqual(list((self.stack / 'rollback').iterdir()), [])
        self.assertTrue(list((self.stack / 'backups/rollback').glob('*.tar.age')))
        # The postgres container user must read init files even though the installer extracts with umask 077.
        initdb = self.stack / 'releases' / NEW / 'postgres/initdb'
        for path, bits in ((initdb.parent, 0o055), (initdb, 0o055), (initdb / '10-porchlight.sh', 0o044)):
            self.assertEqual(stat.S_IMODE(path.stat().st_mode) & 0o077, bits, path)
        calls = self.records()
        dump = next(x for x in calls if 'pg_dump' in x['argv'])
        self.assertEqual(dump['argv'][-5:], ['pg_dump', '-U', 'postgres', '-Fc', 'porchlight'])
        self.assertEqual((dump['dump_mode'], dump['dir_mode']), (0o600, 0o700))
        self.assertFalse(any('pg_restore' in x['argv'] for x in calls))
        wrapper = self.stack / 'porchlight-upgrade'
        self.assertEqual(stat.S_IMODE(wrapper.stat().st_mode), 0o750)
        args = shlex.split(wrapper.read_text().splitlines()[1])
        self.assertEqual(args, ['exec', 'bash', str(self.stack / 'release-current/install.sh'), 'upgrade', '--release', '${1:-latest}',
                                '--root', str(self.stack), '--run-dir', str(self.run_dir), '--project', 'porchlight-fixture'])
        values = dict(x.split('=', 1) for x in (self.stack / '.env').read_text().splitlines() if '=' in x)
        self.assertEqual(values['PORCHLIGHT_UPGRADE_COMMAND'], str(wrapper))
        self.assertEqual(values['PORCHLIGHT_RELEASE_PUBLIC_KEY'], 'FIXTURE-PINNED-PUBLIC-KEY')
        self.assertEqual(values['PORCHLIGHT_VERSION'], NEW)
        # Execute the shell-quoted wrapper, not just inspect its text.
        for version in ([], [NEW]):
            result = subprocess.run([str(wrapper), *version], env=self.env, capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('PASS already at ' + NEW, result.stdout)
        self.assertFalse(list(self.stack.rglob('__pycache__')))

    def test_rollback_migration_health_and_start(self):
        for failure in ('migrate', 'health', 'start', 'load'):
            with self.subTest(failure=failure):
                result = self.run_install('--release', 'latest', expected=1, env={'FAIL_AT': failure})
                self.assertIn('ROLLED BACK to ' + OLD, result.stderr)
                self.assertEqual((self.stack / '.env').read_bytes(), self.before)
                self.assertEqual(os.readlink(self.stack / 'release-current'), 'releases/' + OLD)
                # A completed rollback removes its own dump, so a retry needs no manual cleanup.
                self.assertFalse((self.stack / 'rollback' / ('pre-upgrade-' + OLD + '.dump')).exists())
                restores = [x for x in self.records() if 'pg_restore' in x['argv']]
                self.assertEqual(len(restores), 0 if failure == 'load' else 1)
                if restores:
                    self.assertEqual(restores[0]['version'], OLD)
                    self.assertEqual(restores[0]['dump'], 'plain rollback fixture\n')
                calls = self.records()
                self.assertTrue(any(x['version'] == OLD and x['argv'][-1] == 'healthcheck' for x in calls))
                self.assertIn('Incoming release retained:', result.stderr)
                self.assertTrue(list((self.stack / 'incoming').glob('install.*/release')))
                self.assertTrue(list((self.stack / 'tmp').glob('install.*/rollback.log')))
                (self.p / 'docker.jsonl').unlink()

    def test_admin_repo_and_wrapper_survive_rollback(self):
        (self.stack / '.env').write_bytes(self.before.replace(b'example-owner/porchlight', b'example-owner/admin-porchlight'))
        self.before = (self.stack / '.env').read_bytes()
        wrapper = self.stack / 'porchlight-upgrade'
        wrapper.write_text('#!/bin/bash\nexit 0\n')
        wrapper.chmod(0o750)
        result = self.run_install('--source', str(self.release), expected=1, env={'FAIL_AT': 'migrate'})
        self.assertIn('ROLLED BACK', result.stderr)
        self.assertEqual((self.stack / '.env').read_bytes(), self.before)
        self.assertEqual(wrapper.read_text(), '#!/bin/bash\nexit 0\n')
        self.assertEqual(stat.S_IMODE(wrapper.stat().st_mode), 0o750)
        self.run_install('--source', str(self.release))
        self.assertIn('PORCHLIGHT_RELEASE_REPO=example-owner/admin-porchlight\n', (self.stack / '.env').read_text())

    def test_private_directories_reject_symlinks(self):
        outside = self.p / 'outside'
        outside.mkdir()
        for name in ('incoming', 'rollback'):
            with self.subTest(name=name):
                link = self.stack / name
                link.symlink_to(outside, target_is_directory=True)
                args = ('--release', NEW) if name == 'incoming' else ('--source', str(self.release))
                result = self.run_install(*args, expected=1)
                self.assertIn(name + ' directory must not be a symlink', result.stderr)
                self.assertEqual(list(outside.iterdir()), [])
                self.assertEqual((self.stack / '.env').read_bytes(), self.before)
                link.unlink()

    def test_failed_rollback_guidance(self):
        result = self.run_install('--release', NEW, expected=1, env={'FAIL_AT': 'migrate', 'FAIL_ROLLBACK': '1'})
        self.assertIn('ROLLBACK FAILED', result.stderr)
        self.assertIn('previous trusted installer restore', result.stderr)
        self.assertNotIn('ROLLED BACK to', result.stderr)
        self.assertFalse(any(x['argv'][-1] == 'healthcheck' for x in self.records()))
        dump = self.stack / 'rollback' / ('pre-upgrade-' + OLD + '.dump')
        self.assertEqual(dump.read_text(), 'plain rollback fixture\n')  # kept for manual recovery
        (self.p / 'docker.jsonl').unlink()
        retry = self.run_install('--source', str(self.release), expected=2)
        self.assertIn('recovery dump from a failed rollback exists', retry.stderr)
        self.assertFalse((self.p / 'docker.jsonl').exists() and any('stop' in x['argv'] for x in self.records()))
        self.assertEqual(dump.read_text(), 'plain rollback fixture\n')

    def test_server_and_upgrade_settings_defaults(self):
        password = self.p / 'admin.pass'
        password.write_text('fixture-only')
        for repo in ('example-owner/porchlight', None):
            with self.subTest(repo=repo):
                source = self.p / ('dist-' + OLD)
                record = json.loads((source / 'release.json').read_text())
                record['source']['github'] = repo
                (source / 'release.json').write_text(json.dumps(record, indent=1, sort_keys=True) + '\n')
                self.sign(source, OLD)
                fresh = self.p / ('fresh-' + str(repo is None))
                self.run_install('--source', str(source), '--backup-recipient', 'age1fixture', '--admin-name', 'Sam Rivera',
                                 '--admin-password-file', str(password), mode='server', stack=fresh,
                                 env={'ENROLL_CIDRS': '192.168.77.0/24'})
                text = (fresh / '.env').read_text()
                self.assertEqual('PORCHLIGHT_RELEASE_REPO=' in text, repo is not None)
                self.assertIn('PORCHLIGHT_RELEASE_PUBLIC_KEY=', text)
                self.assertTrue((fresh / 'porchlight-upgrade').is_file())
        (self.stack / '.env').write_bytes(self.before.replace(b'PORCHLIGHT_RELEASE_REPO=example-owner/porchlight\n', b''))
        self.run_install('--source', str(self.release))
        self.assertIn('PORCHLIGHT_RELEASE_REPO=example-owner/porchlight\n', (self.stack / '.env').read_text())


if __name__ == '__main__':
    unittest.main(verbosity=2)
