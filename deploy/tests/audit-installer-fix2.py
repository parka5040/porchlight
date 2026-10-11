#!/usr/bin/env python3
"""Interrupted init acknowledgement: preserve delivery, reconcile server staging."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('fix1_fixture', ROOT / 'deploy/tests/audit-installer-fix1.py')
BASE = importlib.util.module_from_spec(spec)
spec.loader.exec_module(BASE)


class InstallerFix2(unittest.TestCase):
    setUp = BASE.InstallerFix1.setUp
    make_release = BASE.InstallerFix1.make_release
    sign = BASE.InstallerFix1.sign
    run_install = BASE.InstallerFix1.run_install
    records = BASE.InstallerFix1.records
    server_args = BASE.InstallerFix1.server_args
    patch_docker = BASE.InstallerFix1.patch_docker

    def interrupted(self, boundary):
        self.patch_docker("if os.environ.get('FAIL_INIT'): sys.exit(1)", """stage=root/'data/.init-staging'
 if stage.exists():
  # Model initialize/recover: refuse foreign staging, publish, then finish_init.
  if (root/'data/pki/ca.crt').read_text()!='fixture': sys.exit(1)
  import shutil
  output=stage/'init-output.json'
  if not output.exists():
   shutil.rmtree(stage)
   sys.exit(2)  # AlreadyInitialized, after acknowledgement cleanup
  sys.stdout.write(output.read_text());sys.stdout.flush()
  shutil.rmtree(stage)
  sys.exit(0)
 if os.environ.get('FAIL_INIT'): sys.exit(1)""")
        self.patch_docker("if a[0]=='load':", """if 'init' in a and os.environ.get('INTERRUPT_ACK'):
 stage=root/'data/.init-staging';stage.mkdir(parents=True)
 output=stage/'init-output.json'
 output.write_text(json.dumps({'household_id':'fixture','bootstrap_token':'PRIVATE-FIXTURE-NEVER-PRINT'})+'\\n')
 sys.stdout.flush()
 if os.environ['INTERRUPT_ACK']=='after': output.unlink()
 import signal
 os.killpg(os.getpgrp(),signal.SIGKILL)
if a[0]=='load':""")
        password = self.p / 'admin.pass'
        password.write_text('fixture orchard river')
        fresh = self.p / 'fresh'
        command = ['bash', str(self.installer), 'server', *self.server_args(password),
                   '--root', str(fresh), '--run-dir', str(self.run_dir), '--project', 'porchlight-fixture']
        killed = subprocess.run(command, env=dict(self.env, ENROLL_CIDRS='192.168.77.0/24', INTERRUPT_ACK=boundary),
                                capture_output=True, timeout=30, start_new_session=True)
        self.assertEqual(killed.returncode, -9)
        delivered = (fresh / 'init-attempt.json').read_bytes()
        self.assertTrue(bool(json.loads(delivered)['bootstrap_token']))
        return fresh, command, delivered

    def resume(self, boundary):
        fresh, command, delivered = self.interrupted(boundary)
        result = subprocess.run(command, env=dict(self.env, ENROLL_CIDRS='192.168.77.0/24'),
                                capture_output=True, timeout=30)
        self.assertEqual(result.returncode, 0, 'init acknowledgement did not resume')
        self.assertTrue((fresh / 'init.json').read_bytes() == delivered, 'original delivery was replaced')
        self.assertFalse((fresh / 'data/.init-staging').exists(),
                         'successful init resume left hidden plaintext bootstrap delivery')
        self.assertFalse((fresh / 'init-attempt.json').exists())
        self.assertFalse((fresh / 'init.pending.json').exists())
        self.assertEqual(sum('init' in row['argv'] for row in self.records()), 2,
                         'resume skipped server publication/fingerprint checks')

    def test_rr02_resume_before_staged_output_removal(self):
        self.resume('before')

    def test_rr02_resume_after_staged_output_removal(self):
        self.resume('after')

    def test_rr02_resume_refuses_foreign_staging(self):
        fresh, command, delivered = self.interrupted('before')
        (fresh / 'data/pki/ca.crt').write_text('foreign installation')
        result = subprocess.run(command, env=dict(self.env, ENROLL_CIDRS='192.168.77.0/24'),
                                capture_output=True, timeout=30)
        self.assertNotEqual(result.returncode, 0, 'foreign initialization was acknowledged')
        self.assertTrue((fresh / 'init.pending.json').read_bytes() == delivered)
        self.assertTrue((fresh / 'data/.init-staging/init-output.json').exists())


if __name__ == '__main__':
    unittest.main()
