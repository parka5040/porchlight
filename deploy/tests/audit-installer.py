#!/usr/bin/env python3
"""Installer audit regressions: generated installer, private fixtures, no network/Docker."""
import fcntl
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


BASE = module('upgrade_fixture', ROOT / 'deploy/tests/github-upgrade.py')
OPS = BASE.OPS
EXPORT = module('export_fixture', ROOT / 'deploy/lib/verify-export.py')
OLD, NEW = BASE.OLD, BASE.NEW
# Reuse signed-release construction only, not its loopback HTTP server or tests.
class Installer(unittest.TestCase):
    make_release = BASE.UpgradeTests.make_release
    sign = BASE.UpgradeTests.sign
    run_install = BASE.UpgradeTests.run_install
    records = BASE.UpgradeTests.records

    def setUp(self):
        (ROOT / 'var/tests').mkdir(parents=True, exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT / 'var/tests', prefix='audit-installer.')
        self.addCleanup(self.tmp.cleanup)
        self.p = Path(self.tmp.name)
        self.stack = self.p / 'stack with spaces'
        self.run_dir = self.p / 'run'
        self.stack.mkdir()
        self.release = self.make_release(NEW)
        old = self.make_release(OLD)
        shutil.copytree(old, self.stack / 'releases' / OLD)
        (self.stack / 'release-current').symlink_to('releases/' + OLD)
        (self.stack / 'data/pki').mkdir(parents=True)
        (self.stack / '.env').write_text('PORCHLIGHT_VERSION=' + OLD + '\nENROLL_HOST=old.lan:8444\n')
        self.installer = old / 'install.sh'
        self.bin = self.p / 'bin'
        self.bin.mkdir()
        docker = BASE.DOCKER.replace("if a[0]=='load':", """if 'server-cert' in a:
 print(json.dumps({'current':{'dns':['new.lan'],'ips':[]}}))
if 'up' in a and a[-1] in ('api','backup'):
 service=a[-1]
 if os.environ.get('KILL_BEFORE')==service:
  import signal
  os.killpg(os.getpgrp(),signal.SIGKILL)
  sys.exit(1)
 if os.environ.get('SIGNAL_AUX')==service:
  import signal
  os.kill(os.getppid(),signal.SIGTERM)
  sys.exit(1)
 if os.environ.get('FAIL_AUX')==service: sys.exit(1)
 (p/(service+'-running')).write_text(e.get('ENROLL_HOST',''))
if '--exit-code-from' in a and os.environ.get('WAIT_MIGRATE'):
 import time
 (p/'winner-paused').touch()
 for _ in range(200):
  if (p/'release-winner').exists(): break
  time.sleep(.05)
 else: sys.exit(1)
if 'init' in a:
 if os.environ.get('FAIL_INIT'): sys.exit(1)
 (root/'data/pki').mkdir(parents=True,exist_ok=True)
 (root/'data/pki/ca.crt').write_text('fixture')
 print(json.dumps({'household_id':'fixture','bootstrap_token':'PRIVATE-FIXTURE-NEVER-PRINT'}))
if a[0]=='load':""")
        for name, content in [('docker', docker), ('minisign', BASE.MINISIGN), ('age', '''#!/usr/bin/env -S python3 -I -B
import pathlib,shutil,sys
a=sys.argv[1:];shutil.copyfile(a[-1],a[a.index('-o')+1])
''')]:
            (self.bin / name).write_text(content)
            (self.bin / name).chmod(0o755)
        self.env = dict(os.environ, PATH=str(self.bin) + ':/usr/bin:/bin', FIXTURE=str(self.p),
                        HOME=str(self.p / 'home'), XDG_STATE_HOME=str(self.p / 'state'),
                        XDG_DATA_HOME=str(self.p / 'data'), XDG_CONFIG_HOME=str(self.p / 'config'))
        for key in ('PORCHLIGHT_UID', 'PORCHLIGHT_GID'):
            self.env.pop(key, None)

    def test_s1_04_auxiliary_failure_never_restores_acknowledged_writes(self):
        result = self.run_install('--source', str(self.release), expected=1, env={'FAIL_AUX': 'backup'})
        self.assertFalse(any('pg_restore' in x['argv'] for x in self.records()),
                         'destructive restore after new API health')
        self.assertEqual(os.readlink(self.stack / 'release-current'), 'releases/' + NEW)
        self.assertIn('FORWARD RECOVERY', result.stderr)
        self.assertIn('Recovery command:', result.stderr)
        self.assertNotIn('ROLLED BACK', result.stderr)
        self.assertTrue((self.p / 'api-running').exists())
        self.assertFalse(list((self.stack / 'rollback').glob('*.dump')))

    def test_s1_04_signal_after_exposure_recovers_forward(self):
        result = self.run_install('--source', str(self.release), expected=143, env={'SIGNAL_AUX': 'backup'})
        self.assertIn('FORWARD RECOVERY', result.stderr)
        self.assertFalse(any('pg_restore' in x['argv'] for x in self.records()))
        self.assertEqual(os.readlink(self.stack / 'release-current'), 'releases/' + NEW)

    def test_s1_05_lock_survives_verified_handoff_and_blocks_loser(self):
        assets = self.p / 'assets'; assets.mkdir()
        for name in ('manifest.json', 'manifest.json.minisig'):
            shutil.copyfile(self.release / name, assets / name)
        with tarfile.open(assets / f'porchlight-{NEW}-dist.tar', 'w') as tar:
            for path in sorted(self.release.rglob('*')):
                if path.is_file(): tar.add(path, arcname=path.relative_to(self.release).as_posix())
        # Replace transport only; the signature/inventory checks and verified exec are real.
        text = self.installer.read_text().replace('case $mode in\n  desktop)',
                'fetch_release_asset() { cp "$FIXTURE/assets/$2" "$3"; }\ncase $mode in\n  desktop)')
        self.installer.write_text(text)
        command = ['bash', str(self.installer), 'upgrade', '--release', NEW, '--repo', 'example-owner/porchlight',
                   '--root', str(self.stack), '--run-dir', str(self.run_dir)]
        winner = subprocess.Popen(command, env=dict(self.env, WAIT_MIGRATE='1'), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            for _ in range(200):
                if (self.p / 'winner-paused').exists() or winner.poll() is not None: break
                time.sleep(.05)
            self.assertTrue((self.p / 'winner-paused').exists(), 'winner did not reach migration after handoff')
            before = self.records()
            result = self.run_install('--source', str(self.release), expected=2)
            self.assertIn('installer already running', result.stderr)
            self.assertEqual(self.records(), before, 'loser interfered with winning migration')
        finally:
            (self.p / 'release-winner').touch()
            stdout, stderr = winner.communicate(timeout=20)
        self.assertEqual(winner.returncode, 0, stderr)
        self.assertIn('PASS upgrade ' + NEW, stdout)

    def test_s1_05_stack_lock_rejects_all_mutators_without_docker(self):
        with (self.stack / '.installer.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            for mode in ('upgrade', 'server', 'restore', 'enroll-host', 'server-cert', 'funnel', 'backup'):
                with self.subTest(mode=mode):
                    args = ['--source', str(self.release)]
                    if mode == 'restore': args.insert(0, str(self.p / 'bundle'))
                    result = subprocess.run(['bash', str(self.installer), mode, *args, '--root', str(self.stack),
                                             '--run-dir', str(self.run_dir)], env=self.env, capture_output=True, text=True, timeout=20)
                    self.assertNotEqual(result.returncode, 0, 'concurrent mutator succeeded')
                    self.assertIn('installer already running', result.stderr)
                    self.assertEqual(self.records(), [], 'lock loser touched Docker')

    def desktop_release(self):
        archive = self.release / f'porchlight-desktop-{NEW}-linux-x86_64.tar.gz'
        with tarfile.open(archive, 'w:gz') as tar:
            for name, raw in [('porchlight/bin/porchlight', b'#!/bin/sh\nexit 0\n'),
                              ('porchlight/lib/runtime', b'complete runtime')]:
                info = tarfile.TarInfo(name); info.size = len(raw); info.mode = 0o755
                tar.addfile(info, io.BytesIO(raw))
        self.sign(self.release, NEW)

    def test_s1_03_partial_desktop_target_is_not_completion(self):
        self.desktop_release()
        base = self.p / 'data/porchlight'
        target = base / 'releases' / NEW
        target.mkdir(parents=True)
        shutil.copyfile(self.release / 'manifest.json', target / 'release-manifest.json')
        self.run_install('--source', str(self.release), mode='desktop')
        self.assertTrue((base / 'current/app/bin/porchlight').is_file(), 'partial desktop target activated')
        self.assertTrue((base / 'current/install.sh').is_file())
        (base / 'current/app/lib/runtime').unlink()
        self.run_install('--source', str(self.release), mode='desktop')
        self.assertTrue((base / 'current/app/lib/runtime').is_file(), 'same-version retry skipped full validation')

    def make_bundle(self):
        source = self.p / 'bundle-tree'
        for name in ('db.dump', '.env', 'pki/ca.crt', 'pki/ca.key', 'secrets/idempotency.key', 'backup-recipient.txt'):
            path = source / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_text('fixture')
        (source / '.env').write_text(f'PORCHLIGHT_VERSION={NEW}\nPORCHLIGHT_UID={os.getuid()+100}\nPORCHLIGHT_GID={os.getgid()+100}\n')
        (source / 'blobs').mkdir()
        OPS.manifest(source, NEW)
        bundle = self.p / 'bundle.tar'
        with tarfile.open(bundle, 'w') as tar:
            for path in sorted(source.rglob('*')):
                tar.add(path, arcname=path.relative_to(source).as_posix(), recursive=False)
        identity = self.p / 'identity'; identity.write_text('fixture')
        return bundle, identity

    def test_s1b_02_restore_persists_target_identity(self):
        bundle, identity = self.make_bundle()
        self.run_install(str(bundle), '--identity', str(identity), '--source', str(self.release),
                         mode='restore', stack=self.p / 'restored')
        restored = self.p / 'restored'
        e = dict(line.split('=', 1) for line in (restored / '.env').read_text().splitlines())
        self.assertTrue(e['PORCHLIGHT_UID'] == str(os.getuid()), 'restore retained source UID')
        self.assertTrue(e['PORCHLIGHT_GID'] == str(os.getgid()), 'restore retained source GID')
        for name in ('data', 'backups', 'backup-work'):
            self.assertEqual((restored / name).stat().st_mode & 0o777, 0o700)

    def test_s1b_02_restore_rejects_inaccessible_override_before_down(self):
        bundle, identity = self.make_bundle()
        result = self.run_install(str(bundle), '--identity', str(identity), '--source', str(self.release), '--force',
                                 mode='restore', expected=2, env={'PORCHLIGHT_UID': str(os.getuid()+100)})
        self.assertIn('target runtime identity', result.stderr)
        self.assertFalse(any('down' in x['argv'] or 'dropdb' in x['argv'] for x in self.records()))

    def server_args(self, password):
        return ('--source', str(self.release), '--backup-recipient', 'age1' + 'q'*58, '--admin-name', 'Sam Rivera',
                '--admin-password-file', str(password))

    def test_s1b_01_failed_install_resumes_same_credentials(self):
        password = self.p / 'admin.pass'; password.write_text('fixture orchard river')
        fresh = self.p / 'fresh'
        self.run_install(*self.server_args(password), mode='server', stack=fresh, expected=1,
                         env={'ENROLL_CIDRS': '192.168.77.0/24', 'FAIL_AT': 'load'})
        before = (fresh / '.env').read_bytes()
        self.assertTrue((fresh / '.provisioning').exists(), 'missing durable provisioning marker')
        self.run_install(*self.server_args(password), mode='server', stack=fresh,
                         env={'ENROLL_CIDRS': '192.168.77.0/24'})
        self.assertTrue(all(line in (fresh / '.env').read_bytes().splitlines() for line in before.splitlines()),
                        'retry rotated provisioning credentials')
        self.assertFalse((fresh / '.provisioning').exists())

    def test_s1b_01_rejected_init_and_post_init_failure_resume_once(self):
        password = self.p / 'admin.pass'; password.write_text('fixture orchard river')
        fresh = self.p / 'fresh'
        self.run_install(*self.server_args(password), mode='server', stack=fresh, expected=1,
                         env={'ENROLL_CIDRS': '192.168.77.0/24', 'FAIL_INIT': '1'})
        self.run_install(*self.server_args(password), mode='server', stack=fresh, expected=1,
                         env={'ENROLL_CIDRS': '192.168.77.0/24', 'FAIL_AUX': 'backup'})
        before = len([x for x in self.records() if 'init' in x['argv']])
        original = (fresh / 'init.json').read_bytes()
        self.run_install(*self.server_args(password), mode='server', stack=fresh,
                         env={'ENROLL_CIDRS': '192.168.77.0/24'})
        self.assertEqual(len([x for x in self.records() if 'init' in x['argv']]), before, 'completed init was run twice')
        self.assertTrue((fresh / 'init.json').read_bytes() == original, 'initial output changed')

    def test_s1b_01_password_rejected_before_installed_env(self):
        password = self.p / 'admin.pass'; password.write_text('short')
        fresh = self.p / 'fresh'
        result = subprocess.run(['bash', str(self.installer), 'server', *self.server_args(password),
                                 '--root', str(fresh), '--run-dir', str(self.run_dir)],
                                env=dict(self.env, ENROLL_CIDRS='192.168.77.0/24'), capture_output=True, timeout=20)
        self.assertNotEqual(result.returncode, 0, 'invalid initial password accepted')
        self.assertFalse((fresh / '.env').exists(), 'invalid password published final environment')
        password.write_text('fixture orchard river')
        self.run_install(*self.server_args(password), mode='server', stack=fresh,
                         env={'ENROLL_CIDRS': '192.168.77.0/24'})
        self.assertEqual(len([x for x in self.records() if 'init' in x['argv']]), 1)

    def test_s1b_03_matching_host_reconciles_killed_recreation(self):
        for service in ('api', 'backup'):
            with self.subTest(service=service):
                (self.stack / '.env').write_text(f'PORCHLIGHT_VERSION={OLD}\nENROLL_HOST=old.lan:8444\n')
                result = subprocess.run(['bash', str(self.installer), 'enroll-host', '--host', 'new.lan',
                                         '--root', str(self.stack), '--run-dir', str(self.run_dir)],
                                        env=dict(self.env, KILL_BEFORE=service), capture_output=True, text=True, timeout=20,
                                        start_new_session=True)
                self.assertNotEqual(result.returncode, 0)
                before = len(self.records())
                self.run_install('--host', 'new.lan', mode='enroll-host')
                recreated = {x['argv'][-1] for x in self.records()[before:] if 'up' in x['argv']}
                self.assertEqual(recreated, {'api', 'backup'}, 'matching desired host skipped crash reconciliation')


class Archives(unittest.TestCase):
    def setUp(self):
        (ROOT / 'var/tests').mkdir(parents=True, exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT / 'var/tests', prefix='audit-archive.')
        self.addCleanup(self.tmp.cleanup)
        self.p = Path(self.tmp.name)

    def test_s1_02_unsigned_members_rejected_before_extraction(self):
        source = self.p / 'source'; source.mkdir()
        (source / 'payload').write_bytes(b'good')
        OPS.manifest(source, NEW)
        (source / 'manifest.json.minisig').write_text('fixture')
        for kind in ('unlisted', 'wrong-size', 'many', 'pax-size'):
            with self.subTest(kind=kind):
                archive = self.p / 'dist.tar'
                with tarfile.open(archive, 'w:gz') as tar:
                    for path in source.iterdir():
                        if kind != 'wrong-size' or path.name != 'payload': tar.add(path, arcname=path.name)
                    for index in range(100 if kind == 'many' else 1):
                        info = tarfile.TarInfo('payload' if kind == 'wrong-size' else f'extra-{index}')
                        raw = b'\0' * 65536; info.size = len(raw)
                        if kind == 'pax-size': info.type = tarfile.XHDTYPE
                        tar.addfile(info, io.BytesIO(raw))
                target = self.p / kind
                result = subprocess.run(['python3', '-I', '-B', str(ROOT / 'deploy/lib/operations.py'), 'extract',
                                         str(archive), str(target), str(source / 'manifest.json')], capture_output=True)
                self.assertNotEqual(result.returncode, 0, 'unsigned archive inventory accepted')
                self.assertFalse(target.exists(), 'archive wrote members before inventory rejection')

    def test_s1b_04_tar_terminator_is_required(self):
        out = io.BytesIO()
        with tarfile.open(fileobj=out, mode='w') as tar:
            for name, value in [('data.json', {'format_version':2, 'tables':{}}),
                                ('complete.json', {'format_version':2, 'complete':True, 'table_rows':{}, 'blob_count':0, 'blob_sha256':[]})]:
                raw = json.dumps(value).encode(); info = tarfile.TarInfo(name); info.size = len(raw)
                tar.addfile(info, io.BytesIO(raw))
        body = out.getvalue()[:2048]
        path = self.p / 'export.gz'
        for ending in (b'', b'\0'*512, b'invalid'.ljust(1024, b'\0')):
            with self.subTest(ending_length=len(ending)):
                path.write_bytes(gzip.compress(body + ending))
                with self.assertRaises(ValueError, msg='missing or corrupt tar terminator accepted'):
                    EXPORT.verify(path)
        path.write_bytes(gzip.compress(body + b'\0'*1024))
        self.assertEqual(EXPORT.verify(path), {})


if __name__ == '__main__':
    unittest.main(verbosity=2)
