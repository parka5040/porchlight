#!/usr/bin/env python3
"""Final deployment review: offline generated-installer and rollout regressions."""
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('final_fixture', ROOT / 'deploy/tests/audit-installer-fix1.py')
BASE = importlib.util.module_from_spec(spec)
spec.loader.exec_module(BASE)


class FinalDeploy(unittest.TestCase):
    setUp = BASE.InstallerFix1.setUp
    make_release = BASE.InstallerFix1.make_release
    sign = BASE.InstallerFix1.sign
    run_install = BASE.InstallerFix1.run_install
    records = BASE.InstallerFix1.records
    server_args = BASE.InstallerFix1.server_args
    patch_docker = BASE.InstallerFix1.patch_docker
    make_bundle = BASE.BASE.Installer.make_bundle

    def test_fr_deploy_01_source_refuses_different_installer(self):
        # Model differing bootstrap code, not a frozen 0.1.6 release.
        self.installer.write_text(self.installer.read_text() + '\n# previous bootstrap\n')
        result = subprocess.run(['bash', str(self.installer), 'upgrade', '--source', str(self.release),
                                 '--root', str(self.stack), '--run-dir', str(self.run_dir)],
                                env=self.env, capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 2, 'different bootstrap performed upgrade --source itself')
        self.assertIn('new release installer', result.stderr)
        self.assertFalse(self.records(), 'refusal must precede Docker')

    def test_fr_deploy_01_docs_select_new_installer(self):
        text = (ROOT / 'docs/operations/upgrade.md').read_text()
        self.assertNotIn('bash dist/0.1.6/install.sh upgrade --source dist/0.1.7', text,
                         'runbook executes the old installer for a new release')
        for name in ('README.md', 'docs/operations/install.md', 'docs/operations/server-install.md'):
            text = (ROOT / name).read_text()
            self.assertTrue('0.1.6' in text and 'scripts/deploy-server.sh upgrade' in text,
                            name + ' lacks the first-hop controller guidance')

    def test_fr_deploy1_02_first_hop_warning_and_release_notes(self):
        for name in ('docs/operations/upgrade.md', 'scripts/publish-release.sh'):
            text = (ROOT / name).read_text()
            self.assertTrue('0.1.6' in text and 'unsigned' in text and 'scripts/deploy-server.sh upgrade' in text,
                            name + ' lacks the 0.1.6 unsigned extraction warning')

    def agent_model(self):
        self.patch_docker("if 'server-cert' in a:", """state=p/'agent-state.json'
login,connected=json.loads(state.read_text()) if state.exists() else (True,True)
sql=a[-1] if '-c' in a else ''
if 'ALTER ROLE porchlight_agent NOLOGIN' in sql:
 login=False
 if os.environ.get('SIGNAL_GATE'):
  state.write_text(json.dumps([login,connected]))
  import signal
  os.kill(os.getppid(),signal.SIGTERM);sys.exit(1)
if 'pg_terminate_backend' in sql: connected=False
if 'ALTER ROLE porchlight_agent LOGIN' in sql: login=True
state.write_text(json.dumps([login,connected]))
if a[-2:]==['backup','now'] or 'pg_dump' in a or 'pg_restore' in a or '--exit-code-from' in a or a[-1]=='healthcheck':
 event='backup' if a[-2:]==['backup','now'] else ('dump' if 'pg_dump' in a else ('restore' if 'pg_restore' in a else ('migrate' if '--exit-code-from' in a else 'health')))
 with (p/'agent-events.jsonl').open('a') as f: f.write(json.dumps([event,login,connected])+'\\n')
 if event=='dump' and (login or connected): (p/'agent-acknowledged').touch()
 if event=='restore' and (p/'agent-acknowledged').exists(): (p/'agent-lost').touch()
if 'server-cert' in a:""")

    def agent_events(self):
        return [json.loads(line) for line in (self.p / 'agent-events.jsonl').read_text().splitlines()]

    def test_fr_db_01_agent_fenced_through_failed_migration_and_rollback(self):
        self.agent_model()
        self.run_install('--source', str(self.release), expected=1, env={'FAIL_AT': 'migrate'})
        events = self.agent_events()
        self.assertTrue(all(not login and not connected for _, login, connected in events),
                        'SQL agent can commit between snapshot and rollback')
        self.assertTrue({'backup', 'dump', 'migrate', 'restore', 'health'} <= {row[0] for row in events})
        self.assertFalse((self.p / 'agent-lost').exists(), 'rollback lost an acknowledged SQL commit')
        self.assertEqual(json.loads((self.p / 'agent-state.json').read_text()), [True, False])

    def test_fr_db_01_agent_login_restored_on_commit_and_aux_failure(self):
        self.agent_model()
        self.run_install('--source', str(self.release), expected=1, env={'FAIL_AUX': 'backup'})
        self.assertTrue(all(not login and not connected for _, login, connected in self.agent_events()),
                        'SQL agent unfenced before commit')
        self.assertEqual(json.loads((self.p / 'agent-state.json').read_text()), [True, False])
        self.assertFalse(any('pg_restore' in row['argv'] for row in self.records()))

    def test_fr_db_01_gate_signal_restores_login(self):
        self.agent_model()
        self.run_install('--source', str(self.release), expected=143, env={'SIGNAL_GATE': '1'})
        self.assertTrue(json.loads((self.p / 'agent-state.json').read_text())[0], 'trap left agent NOLOGIN')
        self.assertFalse(any('pg_dump' in row['argv'] for row in self.records()))

    def test_fr_db_01_success_restores_agent_login(self):
        self.agent_model()
        self.run_install('--source', str(self.release))
        self.assertTrue(all(not login and not connected for _, login, connected in self.agent_events()))
        self.assertEqual(json.loads((self.p / 'agent-state.json').read_text()), [True, False])

    def test_fr_db_01_failed_rollback_still_attempts_login_cleanup(self):
        self.agent_model()
        result = self.run_install('--source', str(self.release), expected=1,
                                  env={'FAIL_AT': 'migrate', 'FAIL_ROLLBACK': '1'})
        self.assertIn('ROLLBACK FAILED', result.stderr)
        self.assertEqual(json.loads((self.p / 'agent-state.json').read_text()), [True, False])
        self.assertTrue((self.run_dir / 'maintenance').exists())

    def test_fr_db_01_failed_drain_never_takes_snapshot(self):
        self.agent_model()
        self.patch_docker("if 'pg_terminate_backend' in sql: connected=False",
                          "if 'pg_terminate_backend' in sql: sys.exit(1)")
        self.run_install('--source', str(self.release), expected=1)
        self.assertFalse(any('pg_dump' in row['argv'] or row['argv'][-2:] == ['backup', 'now'] for row in self.records()))
        self.assertTrue(json.loads((self.p / 'agent-state.json').read_text())[0])

    def test_fr_deploy1_01_restore_persists_target_compose_inputs(self):
        bundle, identity = self.make_bundle()
        target = self.p / 'restored'
        overrides = dict(PG_PORT='55432', LAN_PORT='18443', ENROLL_PORT='18444', LAN_BIND='127.0.0.1',
                         ENROLL_HOST='localhost:18444', ENROLL_CIDRS='127.0.0.1/32,172.28.250.1/32',
                         PORCHLIGHT_ENDPOINTS='https://localhost:18443',
                         BACKEND_SUBNET='172.28.250.0/24', INGRESS_SUBNET='172.28.251.0/24')
        self.patch_docker("if 'server-cert' in a:", """if a[0]=='compose':
 keys=('PG_PORT','LAN_PORT','ENROLL_PORT','LAN_BIND','ENROLL_HOST','ENROLL_CIDRS','PORCHLIGHT_ENDPOINTS','BACKEND_SUBNET','INGRESS_SUBNET')
 # Compose's environment-over-env-file interpolation, without exposing credentials.
 with (p/'network.jsonl').open('a') as f: f.write(json.dumps({k:os.environ.get(k,e.get(k)) for k in keys})+'\\n')
if 'server-cert' in a:""")
        self.run_install(str(bundle), '--identity', str(identity), '--source', str(self.release), '--no-start',
                         mode='restore', stack=target, env=overrides)
        records = [json.loads(line) for line in (self.p / 'network.jsonl').read_text().splitlines()]
        self.assertTrue(records and all(row == overrides for row in records),
                        'restored Compose ignores isolated target network overrides')
        saved = dict(line.split('=', 1) for line in (target / '.env').read_text().splitlines())
        self.assertTrue(all(saved.get(k) == v for k, v in overrides.items()), 'target overrides not persisted')

    def test_fr_deploy1_01_invalid_override_rejected_before_replace(self):
        bundle, identity = self.make_bundle()
        before = (self.stack / '.env').read_bytes()
        for key, value in (('LAN_BIND', '$(invalid)'), ('ENROLL_PORT', '65536'),
                           ('BACKEND_SUBNET', 'not-a-network'), ('PORCHLIGHT_ENDPOINTS', 'http://localhost:8443')):
            result = subprocess.run(['bash', str(self.installer), 'restore', str(bundle), '--identity', str(identity),
                                     '--source', str(self.release), '--root', str(self.stack), '--force'],
                                    env=dict(self.env, **{key: value}), capture_output=True, timeout=30)
            self.assertNotEqual(result.returncode, 0, 'invalid restore override accepted')
            self.assertTrue((self.stack / '.env').read_bytes() == before, 'invalid restore override replaced state')
        self.assertFalse(any('down' in row['argv'] or 'dropdb' in row['argv'] for row in self.records()))

    def test_fr_deploy1_04_drill_selects_recovery_tool_separately(self):
        text = (ROOT / 'deploy/scripts/restore-drill.sh').read_text()
        self.assertIn('--installer)', text, 'drill cannot select a newer trusted recovery installer')
        self.assertNotIn('bash "$source/install.sh" restore', text)
        self.assertIn('bash "$installer" restore', text)
        for name in ('docs/operations/upgrade.md', 'docs/operations/backup-restore.md'):
            text = (ROOT / name).read_text()
            self.assertTrue('newest verified installer' in text, name + ' still selects old restore code')

    def test_fr_deploy1_04_new_tool_old_bundle_identity_lock_and_version(self):
        bundle, identity = self.make_bundle()
        tree = self.p / 'bundle-tree'
        # Old-format bundle data and matching old artifacts, with a different source UID/GID.
        (tree / '.env').write_text((tree / '.env').read_text().replace('0.1.7', '0.1.6'))
        BASE.BASE.OPS.manifest(tree, '0.1.6')
        with tarfile.open(bundle, 'w') as tar:
            for path in sorted(tree.rglob('*')):
                tar.add(path, arcname=path.relative_to(tree).as_posix(), recursive=False)
        old = self.p / 'dist-0.1.6'
        (old / 'install.sh').write_text('#!/bin/sh\nexit 99\n')
        self.sign(old, '0.1.6')
        target = self.p / 'restored'; target.mkdir()
        with (target / '.installer.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.run_install(str(bundle), '--identity', str(identity), '--source', str(old), mode='restore',
                             stack=target, installer=self.release / 'install.sh', expected=2)
        self.assertFalse(self.records())
        self.run_install(str(bundle), '--identity', str(identity), '--source', str(self.release), mode='restore',
                         stack=target, installer=self.release / 'install.sh', expected=2)
        self.run_install(str(bundle), '--identity', str(identity), '--source', str(old), '--no-start', mode='restore',
                         stack=target, installer=self.release / 'install.sh')
        saved = dict(line.split('=', 1) for line in (target / '.env').read_text().splitlines())
        self.assertTrue(saved['PORCHLIGHT_UID'] == str(os.getuid()) and saved['PORCHLIGHT_GID'] == str(os.getgid()))
        self.assertEqual(saved['PORCHLIGHT_VERSION'], '0.1.6')

    def test_fr_deploy1_03_numeric_dns_rejected_before_binding_and_correction_succeeds(self):
        password = self.p / 'admin.pass'; password.write_text('fixture orchard river')
        fresh = self.p / 'fresh'
        for name in ('127.0.0.1', '123', '1.2.3'):
            result = subprocess.run(['bash', str(self.installer), 'server', *self.server_args(password),
                                     '--san-dns', name, '--root', str(fresh), '--run-dir', str(self.run_dir)],
                                    env=dict(self.env, ENROLL_CIDRS='192.168.77.0/24'), capture_output=True, timeout=30)
            self.assertFalse((fresh / '.provisioning').exists() or (fresh / '.env').exists(),
                             'numeric DNS SAN bound an unresumable provisioning attempt')
            self.assertNotEqual(result.returncode, 0)
        self.run_install(*self.server_args(password), '--san-ip', '127.0.0.1', mode='server', stack=fresh,
                         env={'ENROLL_CIDRS': '192.168.77.0/24'})


if __name__ == '__main__':
    unittest.main()
