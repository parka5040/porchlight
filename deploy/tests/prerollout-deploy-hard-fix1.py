#!/usr/bin/env python3
"""Recovery regressions with generated signed fixtures; no Docker or network."""
import importlib.util
import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]

def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value

BASE = module('installer', ROOT / 'deploy/tests/audit-installer.py')
HARD = module('hardening', ROOT / 'deploy/tests/prerollout-deploy-hard.py')
OPS = BASE.OPS

class Recovery(unittest.TestCase):
    setUp = BASE.Installer.setUp
    make_release = BASE.Installer.make_release
    sign = BASE.Installer.sign
    run_install = BASE.Installer.run_install
    records = BASE.Installer.records
    make_bundle = BASE.Installer.make_bundle

    def docker_patch(self, text):
        path = self.bin / 'docker'
        path.write_text(path.read_text().replace("if 'server-cert' in a:", text + "\nif 'server-cert' in a:", 1))

    def test_rr01_empty_binding_migration_failure(self):
        with (self.stack / '.env').open('a') as f: f.write('LAN_BIND=\n')
        self.docker_patch("""if '--env-file' in a:
 config=pathlib.Path(a[a.index('--env-file')+1])
 settings=dict(x.split('=',1) for x in config.read_text().splitlines() if '=' in x)
 if '/0.1.7/' in a[a.index('-f')+1] and not settings.get('LAN_BIND'): sys.exit(1)
""")
        result = self.run_install('--source', str(self.release), expected=1, env={'FAIL_AT': 'migrate'})
        self.assertIn('ROLLED BACK', result.stderr, 'RR-01: invalid new interpolation defeated rollback')
        self.assertEqual(os.readlink(self.stack / 'release-current'), 'releases/0.1.6')
        self.assertTrue((self.p / 'api-running').exists())
        self.assertTrue((self.p / 'backup-running').exists())

    def test_rr01_signal_immediately_after_switch(self):
        wrapper = self.bin / 'mv'
        wrapper.write_text('''#!/usr/bin/env -S python3 -I -B
import os,pathlib,signal,subprocess,sys
r=subprocess.run(['/usr/bin/mv',*sys.argv[1:]])
p=pathlib.Path(os.environ['FIXTURE']);root=pathlib.Path(os.environ['PORCHLIGHT_ROOT'])
if sys.argv[-1]==str(root/'release-current') and os.readlink(root/'release-current')=='releases/0.1.7' and not (p/'interrupted').exists():
 (p/'interrupted').touch();os.kill(os.getppid(),signal.SIGTERM)
sys.exit(r.returncode)
''')
        wrapper.chmod(0o755)
        self.docker_patch("""if '--env-file' in a and '/0.1.7/' in a[a.index('-f')+1]:
 settings=dict(x.split('=',1) for x in (root/'.env').read_text().splitlines() if '=' in x)
 assert settings.get('LAN_BIND'), 'RR-01: switched before staging LAN_BIND'
 assert (root/'credentials/app-url').is_file(), 'RR-01: switched before staging credentials'
""")
        result = self.run_install('--source', str(self.release), expected=143)
        self.assertIn('ROLLED BACK', result.stderr, 'RR-01: switch signal defeated rollback')
        self.assertTrue((self.p / 'api-running').exists())
        self.assertTrue((self.p / 'backup-running').exists())

    def test_rr01_signal_after_failed_migration(self):
        with (self.stack/'.env').open('a') as f: f.write('LAN_BIND=\n')
        self.docker_patch("""if v=='0.1.7' and '--exit-code-from' in a:
 import signal
 os.kill(os.getppid(),signal.SIGTERM)
 sys.exit(1)
""")
        result=self.run_install('--source', str(self.release), expected=143)
        self.assertIn('ROLLED BACK', result.stderr)
        self.assertTrue(any('pg_restore' in row['argv'] for row in self.records()))
        self.assertTrue((self.p/'api-running').exists())
        self.assertTrue((self.p/'backup-running').exists())

    def test_rr01_bad_new_config_never_switches(self):
        with (self.stack/'.env').open('a') as f: f.write('LAN_BIND=not-an-address\n')
        self.run_install('--source', str(self.release), expected=1)
        self.assertFalse(any(row['current']=='releases/0.1.7' for row in self.records()))
        self.assertTrue((self.p/'api-running').exists())

    def test_rr02_abrupt_death_after_agent_login_is_forward_only(self):
        self.docker_patch("""if any('ALTER ROLE porchlight_agent LOGIN;' in arg for arg in a) and v=='0.1.7':
 (p/'acknowledged-agent-write').write_text('kept')
 import signal
 os.killpg(os.getpgrp(),signal.SIGKILL)
""")
        command = ['bash', str(self.release/'install.sh'), 'upgrade', '--source', str(self.release), '--root', str(self.stack), '--run-dir', str(self.run_dir)]
        result = subprocess.run(command, env=self.env, capture_output=True, start_new_session=True, timeout=30)
        self.assertEqual(result.returncode, -9)
        self.assertEqual((self.stack/'data/maintenance').read_text(), 'porchlight-upgrade-v1 committed\n', 'RR-02: acknowledged write has no durable forward-only marker')
        retry = self.run_install('--source', str(self.release), expected=2)
        self.assertIn('forward-only', retry.stderr)
        self.assertEqual((self.p/'acknowledged-agent-write').read_text(), 'kept')
        self.assertFalse(any('pg_restore' in row['argv'] for row in self.records()))

    def test_rr02_committed_marker_survives_unlink_sync_death(self):
        with self.tmp_context() as root:
            OPS.durable_fence(root, 'create')
            OPS.durable_fence(root, 'commit')
            self.assertEqual(OPS.fence_phase(root), 'committed')
            with patch.object(OPS.os, 'fsync', side_effect=OSError('simulated death')):
                with self.assertRaises(OSError): OPS.durable_fence(root, 'remove')
            # After reboot the unlink either persisted or the committed inode returns.
            self.assertEqual(OPS.fence_phase(root), 'absent')
            (root/'data/maintenance').write_text('porchlight-upgrade-v1 committed\n')
            self.assertEqual(OPS.fence_phase(root), 'committed')

    def tmp_context(self):
        import contextlib
        import tempfile
        @contextlib.contextmanager
        def tree():
            with tempfile.TemporaryDirectory(dir=ROOT/'var') as directory:
                root=Path(directory);(root/'data').mkdir();yield root
        return tree()

    def test_rr03_binding_refreshes_backup_file_bind(self):
        self.run_install('--lan-bind', '192.168.77.10', mode='lan-bind')
        self.assertTrue(any('up' in row['argv'] and '--force-recreate' in row['argv'] and row['argv'][-1]=='backup' for row in self.records()), 'RR-03: backup retains old env inode')

    def test_rr03_backup_failure_restores_previous_binding(self):
        before=(self.stack/'.env').read_bytes()
        self.run_install('--lan-bind', '192.168.77.10', mode='lan-bind', expected=1, env={'FAIL_AUX':'backup'})
        self.assertTrue((self.stack/'.env').read_bytes()==before, 'binding rollback lost previous configuration')
        self.assertGreaterEqual(sum('up' in row['argv'] and row['argv'][-1]=='backup' for row in self.records()), 2)

    def test_rr04_restore_flag_overrides_bundle(self):
        bundle, identity = self.make_bundle()
        target=self.p/'restored'
        self.run_install(str(bundle), '--identity', str(identity), '--source', str(self.release), '--lan-bind', '192.168.77.10', mode='restore', stack=target)
        self.assertIn('LAN_BIND=192.168.77.10\n', (target/'.env').read_text(), 'RR-04: restore ignored explicit bind')

    def test_rr04_explicit_wildcard_and_flag_precedence(self):
        bundle, identity=self.make_bundle()
        target=self.p/'restored'
        self.run_install(str(bundle), '--identity', str(identity), '--source', str(self.release), '--lan-bind', '0.0.0.0', mode='restore', stack=target, env={'LAN_BIND':'192.168.77.10'})
        settings=dict(line.split('=',1) for line in (target/'.env').read_text().splitlines())
        self.assertEqual(settings['LAN_BIND'], '0.0.0.0')

    def test_rr04_invalid_flag_rejected_before_docker(self):
        self.run_install(str(self.p/'missing'), '--lan-bind', '::', mode='restore', expected=2)
        self.assertFalse(self.records(), 'RR-04: invalid bind touched Docker')

    def test_rr04_legacy_restore_requires_binding(self):
        bundle, identity = self.make_bundle()
        import tarfile
        source=self.p/'bundle-tree'
        path=source/'.env';path.write_text(path.read_text().replace('LAN_BIND=127.0.0.1\n', ''))
        OPS.manifest(source, BASE.NEW)
        with tarfile.open(bundle, 'w') as tar:
            for path in sorted(source.rglob('*')): tar.add(path, arcname=path.relative_to(source).as_posix(), recursive=False)
        result=self.run_install(str(bundle), '--identity', str(identity), '--source', str(self.release), '--force', mode='restore', expected=2)
        self.assertFalse(any('down' in row['argv'] or 'dropdb' in row['argv'] for row in self.records()), 'RR-04: legacy restore changed target before binding decision')
        self.assertIn('binding', result.stderr)

    def test_rr06_negative_control(self):
        body='    environment:\n      POSTGRES_PASSWORD: deliberately-not-printed\n    volumes: []\n'
        with self.assertRaises(AssertionError, msg='RR-06: plaintext secret negative control passed'):
            HARD.assert_no_secret_environment(self, body)

if __name__ == '__main__':
    unittest.main()
