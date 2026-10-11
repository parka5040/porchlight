#!/usr/bin/env python3
"""Installer re-review regressions, without Docker or network."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('audit_fixture', ROOT / 'deploy/tests/audit-installer.py')
BASE = importlib.util.module_from_spec(spec)
spec.loader.exec_module(BASE)


class InstallerFix1(unittest.TestCase):
    setUp = BASE.Installer.setUp
    make_release = BASE.Installer.make_release
    sign = BASE.Installer.sign
    run_install = BASE.Installer.run_install
    records = BASE.Installer.records
    server_args = BASE.Installer.server_args

    def patch_docker(self, before, after):
        path = self.bin / 'docker'
        text = path.read_text()
        assert before in text
        path.write_text(text.replace(before, after))

    def test_rr01_health_window_writes_are_fenced_before_rollback(self):
        self.patch_docker("if 'server-cert' in a:", """if v=='0.1.7' and (('up' in a and a[-1]=='api') or a[-1]=='healthcheck'):
 # Model an API request at both health boundaries; real middleware is tested in Rust.
 status=503 if (pathlib.Path(os.environ['PORCHLIGHT_RUN_DIR'])/'maintenance').exists() else 201
 with (p/'write-status').open('a') as f: f.write(str(status)+'\\n')
 if status==201: (p/'acknowledged-write').touch()
if 'pg_restore' in a and (p/'acknowledged-write').exists(): (p/'lost-write').touch()
if 'server-cert' in a:""")
        self.run_install('--source', str(self.release), expected=1, env={'FAIL_AT': 'health'})
        self.assertEqual((self.p / 'write-status').read_text().splitlines(), ['503', '503'],
                         'health-window mutation was acknowledged while rollback was armed')
        self.assertFalse((self.p / 'lost-write').exists(), 'rollback discarded an acknowledged write')
        self.assertFalse((self.run_dir / 'maintenance').exists(), 'rollback retained fence')

    def test_rr01_forward_recovery_removes_fence(self):
        self.run_install('--source', str(self.release), expected=1, env={'FAIL_AUX': 'backup'})
        self.assertFalse((self.run_dir / 'maintenance').exists())
        self.assertFalse(any('pg_restore' in row['argv'] for row in self.records()))

    def test_rr01_signal_during_health_rolls_back_behind_fence(self):
        self.patch_docker("if 'server-cert' in a:", """if v=='0.1.7' and a[-1]=='healthcheck':
 assert (pathlib.Path(os.environ['PORCHLIGHT_RUN_DIR'])/'maintenance').exists()
 import signal
 os.kill(os.getppid(),signal.SIGTERM)
 sys.exit(1)
if 'server-cert' in a:""")
        result = self.run_install('--source', str(self.release), expected=143)
        self.assertIn('ROLLED BACK', result.stderr)
        self.assertTrue(any('pg_restore' in row['argv'] for row in self.records()))
        self.assertFalse((self.run_dir / 'maintenance').exists())

    def test_rr01_signal_at_unfence_cannot_restore(self):
        # Interrupt exactly before unlink; EXIT must recover forward and remove the fence itself.
        wrapper = self.bin / 'rm'
        wrapper.write_text('''#!/usr/bin/env -S python3 -I -B
import os,pathlib,signal,sys
marker=pathlib.Path(os.environ['FIXTURE'])/'unfence-interrupted'
if sys.argv[-1].endswith('/maintenance') and not marker.exists():
 marker.touch();os.kill(os.getppid(),signal.SIGTERM);sys.exit(1)
os.execv('/usr/bin/rm',['rm',*sys.argv[1:]])
''')
        wrapper.chmod(0o755)
        result = self.run_install('--source', str(self.release), expected=143)
        self.assertIn('FORWARD RECOVERY', result.stderr)
        self.assertFalse(any('pg_restore' in row['argv'] for row in self.records()))
        self.assertFalse((self.run_dir / 'maintenance').exists())

    def test_rr01_stale_fence_reports_before_docker(self):
        self.run_dir.mkdir()
        (self.run_dir / 'maintenance').touch()
        result = self.run_install('--source', str(self.release), expected=2)
        self.assertIn('Stale maintenance fence', result.stderr)
        self.assertFalse(self.records())

    def test_rr02_ack_cleanup_failure_preserves_delivered_output(self):
        self.patch_docker("if os.environ.get('FAIL_INIT'): sys.exit(1)", """if (root/'data/.init-staging').exists():
  # The real server cleans up, then returns AlreadyInitialized without another token.
  import shutil
  shutil.rmtree(root/'data/.init-staging')
  sys.exit(2)
 if os.environ.get('FAIL_INIT'): sys.exit(1)""")
        self.patch_docker("if a[0]=='load':", """if 'init' in a and os.environ.get('FAIL_ACK'):
 (root/'data/.init-staging/pki').mkdir(parents=True)
 sys.stdout.flush()
 sys.exit(1)
if a[0]=='load':""")
        password = self.p / 'admin.pass'; password.write_text('fixture orchard river')
        fresh = self.p / 'fresh'
        self.run_install(*self.server_args(password), mode='server', stack=fresh, expected=1,
                         env={'ENROLL_CIDRS': '192.168.77.0/24', 'FAIL_ACK': '1'})
        delivered = (fresh / 'init.pending.json').read_bytes()
        self.assertTrue(bool(json.loads(delivered)['bootstrap_token']))
        result = subprocess.run(['bash', str(self.installer), 'server', *self.server_args(password),
                                 '--root', str(fresh), '--run-dir', str(self.run_dir), '--project', 'porchlight-fixture'],
                                env=dict(self.env, ENROLL_CIDRS='192.168.77.0/24'), capture_output=True, timeout=30)
        self.assertTrue((fresh / 'init.json').exists() and (fresh / 'init.json').read_bytes() == delivered,
                        'init retry lost the complete delivered bootstrap output')
        self.assertEqual(result.returncode, 0, 'acknowledged init did not resume')
        self.assertFalse((fresh / '.provisioning').exists())

    def test_rr02_killed_delivery_is_recovered_before_another_init(self):
        self.patch_docker("if os.environ.get('FAIL_INIT'): sys.exit(1)", """if (root/'data/.init-staging').exists():
  import shutil
  shutil.rmtree(root/'data/.init-staging')
  sys.exit(2)  # AlreadyInitialized, no output
 if os.environ.get('FAIL_INIT'): sys.exit(1)""")
        self.patch_docker("if a[0]=='load':", """if 'init' in a and os.environ.get('KILL_ACK'):
 (root/'data/.init-staging/pki').mkdir(parents=True)
 sys.stdout.flush()
 import signal
 os.killpg(os.getpgrp(),signal.SIGKILL)
if a[0]=='load':""")
        password = self.p / 'admin.pass'; password.write_text('fixture orchard river')
        fresh = self.p / 'fresh'
        command = ['bash', str(self.installer), 'server', *self.server_args(password),
                   '--root', str(fresh), '--run-dir', str(self.run_dir), '--project', 'porchlight-fixture']
        killed = subprocess.run(command, env=dict(self.env, ENROLL_CIDRS='192.168.77.0/24', KILL_ACK='1'),
                                capture_output=True, timeout=30, start_new_session=True)
        self.assertEqual(killed.returncode, -9)
        delivery = next(path.read_bytes() for path in fresh.rglob('init*json')
                        if json.loads(path.read_bytes()).get('bootstrap_token'))
        retry = subprocess.run(command, env=dict(self.env, ENROLL_CIDRS='192.168.77.0/24'),
                               capture_output=True, timeout=30)
        self.assertEqual(retry.returncode, 0, 'killed init delivery was not recovered')
        self.assertTrue((fresh / 'init.json').read_bytes() == delivery, 'delivered token changed')
        self.assertEqual(sum('init' in row['argv'] for row in self.records()), 2)
        self.assertFalse((fresh / 'init-attempt.json').exists())

    def test_rr03_invalid_storage_zone_rejected_before_binding(self):
        password = self.p / 'admin.pass'; password.write_text('fixture orchard river')
        for zone in ('Factory', 'posix/America/New_York', 'localtime'):
            with self.subTest(zone=zone):
                fresh = self.p / ('fresh-' + zone.replace('/', '-'))
                result = subprocess.run(['bash', str(self.installer), 'server', *self.server_args(password), '--tz', zone,
                                         '--root', str(fresh), '--run-dir', str(self.run_dir)],
                                        env=dict(self.env, ENROLL_CIDRS='192.168.77.0/24', FAIL_INIT='1'),
                                        capture_output=True, timeout=30)
                self.assertFalse((fresh / '.provisioning').exists(), 'server-invalid zone bound provisioning')
                self.assertNotEqual(result.returncode, 0)
                self.run_install(*self.server_args(password), '--tz', 'US/Eastern', mode='server', stack=fresh,
                                 env={'ENROLL_CIDRS': '192.168.77.0/24'})

    def test_rr04_changed_effective_config_rejected_before_services(self):
        password = self.p / 'admin.pass'; password.write_text('fixture orchard river')
        fresh = self.p / 'fresh'
        self.run_install(*self.server_args(password), mode='server', stack=fresh, expected=1,
                         env={'ENROLL_CIDRS': '192.168.77.0/24', 'FAIL_AT': 'load'})
        before = (fresh / '.env').read_bytes()
        for change in ({'ENROLL_CIDRS': '192.168.77.0/25'}, {'LAN_PORT': '9444'}, {'ENROLL_PORT': '9445'},
                       {'PG_PORT': '5433'}, {'LAN_BIND': '192.168.77.2'}, {'ENROLL_HOST': 'new.lan'},
                       {'PORCHLIGHT_ENDPOINTS': 'https://new.lan:8443'}, {'PORCHLIGHT_MIN_CLIENT_VERSION': '0.1.1'},
                       {'BACKEND_SUBNET': '192.168.78.0/24'}, {'INGRESS_SUBNET': '192.168.79.0/24'},
                       {'PORCHLIGHT_UID': str(os.getuid()+1)}, {'PORCHLIGHT_GID': str(os.getgid()+1)}):
            with self.subTest(field=next(iter(change))):
                count = len(self.records())
                result = subprocess.run(['bash', str(self.installer), 'server', *self.server_args(password),
                                         '--root', str(fresh), '--run-dir', str(self.run_dir), '--project', 'porchlight-fixture'],
                                        env=dict(self.env, **({'ENROLL_CIDRS': '192.168.77.0/24'} | change)),
                                        capture_output=True, timeout=30)
                self.assertEqual(result.returncode, 2, 'changed effective deployment input silently accepted')
                self.assertTrue((fresh / '.env').read_bytes() == before, 'persisted configuration changed')
                self.assertFalse(any('load' in row['argv'] or 'up' in row['argv'] for row in self.records()[count:]))
        self.run_install(*self.server_args(password), mode='server', stack=fresh,
                         env={'ENROLL_CIDRS': '192.168.77.0/24'})

    def test_rr04_compose_uses_saved_config_not_inherited_overrides(self):
        self.patch_docker("if 'server-cert' in a:", """if a[0]=='compose' and '--env-file' in a:
 for key in ('PORCHLIGHT_UID','PORCHLIGHT_GID','PG_PORT','LAN_PORT','ENROLL_PORT','LAN_BIND',
             'ENROLL_CIDRS','ENROLL_HOST','PORCHLIGHT_ENDPOINTS','BACKEND_SUBNET','INGRESS_SUBNET',
             'PORCHLIGHT_MIN_CLIENT_VERSION'):
  assert key not in os.environ, 'Compose inherited a persisted interpolation input'
 (p/'saved-config-used').touch()
if 'server-cert' in a:""")
        self.run_install('--source', str(self.release), env={'LAN_PORT': '9444', 'ENROLL_CIDRS': '192.168.77.0/24',
                                                           'PORCHLIGHT_UID': str(os.getuid()+1)})
        self.assertTrue((self.p / 'saved-config-used').exists())


if __name__ == '__main__':
    unittest.main()
