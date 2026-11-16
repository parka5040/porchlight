#!/usr/bin/env python3
"""Pre-rollout deployment policy; real container checks remain controller-only."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('ops', ROOT / 'deploy/lib/operations.py')
ops = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ops)


def services(name):
    text = (ROOT / name).read_text()
    body = text.split('services:\n', 1)[1].split('\nnetworks:', 1)[0]
    return text, dict(re.findall(r'^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)', body, re.M | re.S))


def assert_no_secret_environment(case, body):
    block = re.search(r'^    environment:\n((?:^      [^\n]*\n|^\s*\n)*)', body, re.M)
    if block:
        secret = re.search(r'^      (?:POSTGRES_PASSWORD|PORCHLIGHT_\w+_(?:PASSWORD|URL)):', block[1], re.M)
        case.assertIsNone(secret, 'DP-05: secret in Docker environment')


class Hardening(unittest.TestCase):
    def test_dp01_durable_fence_survives_reboot(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'var') as tmp:
            root = Path(tmp)
            (root / 'data').mkdir()
            (root / 'run').mkdir()
            # Exercise the actual installer helper, then simulate loss of /run.
            installer = (ROOT / 'deploy/install.sh').read_text()
            block = installer.split('create_maintenance() {', 1)[1].split('\n}\n', 1)[0]
            script = 'source deploy/lib/operations.sh\nstackroot=$1; run_dir=$1/run\ncreate_maintenance() {' + block + '\n}\ncreate_maintenance\n'
            result = subprocess.run(['bash', '-euc', script, 'fixture', tmp], cwd=ROOT, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            (root / 'run/maintenance').unlink()
            self.assertTrue((root / 'data/maintenance').is_file(), 'DP-01: reboot lost write fence')
            compose, _ = services('deploy/compose.yml')
            self.assertIn('PORCHLIGHT_MAINTENANCE_FILE: /data/maintenance', compose)
            self.assertIn('self.agent_socket.with_file_name("maintenance")', (ROOT / 'server/api/src/config.rs').read_text())

    def test_dp02_least_privilege(self):
        for name in ('deploy/compose.yml', 'deploy/compose.dev.yml'):
            _, entries = services(name)
            for service in ('api', 'migrate', 'backup'):
                body = entries[service]
                self.assertIn('read_only: true', body, f'DP-02: {name} {service}')
                self.assertIn('cap_drop: [ALL]', body)
                self.assertIn('security_opt: ["no-new-privileges:true"]', body)
                self.assertIn('/tmp:', body)
            self.assertIn('security_opt: ["no-new-privileges:true"]', entries['postgres'])
        _, entries = services('deploy/compose.funnel.yml')
        self.assertIn('cap_drop: [ALL]', entries['tailscale'])

    def test_dp03_binding_is_deliberate(self):
        self.assertTrue(hasattr(ops, 'resolve_lan_bind'), 'DP-03: no LAN bind resolver')
        addresses = ['192.168.77.10', '192.168.78.10']
        self.assertEqual(ops.resolve_lan_bind('', '192.168.77.0/24', addresses), '192.168.77.10')
        with self.assertRaises(ValueError): ops.resolve_lan_bind('0.0.0.0', '192.168.77.0/24', addresses)
        self.assertEqual(ops.resolve_lan_bind('0.0.0.0', '', [], True), '0.0.0.0')
        with self.assertRaises(ValueError): ops.resolve_lan_bind('::', '', [])
        with self.assertRaises(ValueError): ops.resolve_lan_bind('', '192.168.77.0/24', ['192.168.77.10', '192.168.77.11'])
        self.assertIn('NOTICE: existing stack has no LAN_BIND', (ROOT / 'deploy/install.sh').read_text())

    def test_dp04_network_boundary(self):
        text, entries = services('deploy/compose.yml')
        self.assertNotIn('ports:', entries['postgres'], 'DP-04: unused production DB host port')
        self.assertIn('  backend:\n    internal: true', text)
        self.assertIn('networks: [backend, ingress]', entries['api'])
        self.assertNotIn('    internal: true', text.split('  ingress:\n', 1)[1])

    def test_dp05_no_compose_secret_values(self):
        _, entries = services('deploy/compose.yml')
        for body in entries.values():
            assert_no_secret_environment(self, body)
        self.assertIn('PORCHLIGHT_DATABASE_URL_FILE:', entries['api'])
        self.assertIn('PORCHLIGHT_MIGRATOR_URL_FILE:', entries['migrate'])
        self.assertIn('PORCHLIGHT_BACKUP_PASSWORD_FILE:', entries['backup'])
        self.assertTrue(hasattr(ops, 'prepare_credentials'), 'DP-05: no credential provisioning')
        with tempfile.TemporaryDirectory(dir=ROOT / 'var') as tmp:
            root = Path(tmp)
            (root / '.env').write_text('\n'.join(key + '=fixture' for key in ('POSTGRES_PASSWORD', 'PORCHLIGHT_APP_PASSWORD', 'PORCHLIGHT_AGENT_PASSWORD', 'PORCHLIGHT_MIGRATOR_PASSWORD', 'PORCHLIGHT_BACKUP_PASSWORD')))
            ops.prepare_credentials(root)
            for path in (root / 'credentials').iterdir():
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertIn('porchlight_app:fixture@postgres:5432', (root / 'credentials/app-url').read_text())

    def test_dp05_native_file_and_legacy_backup_credentials(self):
        spec = importlib.util.spec_from_file_location('backup', ROOT / 'deploy/backup/backup.py')
        backup = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(backup)
        with tempfile.TemporaryDirectory(dir=ROOT / 'var') as tmp:
            path = Path(tmp) / 'backup-password'
            path.write_text('fixture-file-value\n')
            path.chmod(0o600)
            with patch.dict(os.environ, PORCHLIGHT_BACKUP_PASSWORD='fixture-legacy', PORCHLIGHT_BACKUP_PASSWORD_FILE=str(path)):
                self.assertTrue(backup.db_env()['PGPASSWORD'] == 'fixture-file-value')
                path.chmod(0o644)
                with self.assertRaises(ValueError): backup.db_env()
            with patch.dict(os.environ, PORCHLIGHT_BACKUP_PASSWORD='fixture-legacy'):
                os.environ.pop('PORCHLIGHT_BACKUP_PASSWORD_FILE', None)
                self.assertTrue(backup.db_env()['PGPASSWORD'] == 'fixture-legacy')
            path.chmod(0o600)
            probe = Path(tmp) / 'probe'
            probe.write_text('#!/bin/sh\n[ "$PORCHLIGHT_BACKUP_PASSWORD" = fixture-file-value ] && [ -z "${PORCHLIGHT_BACKUP_PASSWORD_FILE:-}" ]\n')
            probe.chmod(0o755)
            wrapper = (ROOT / 'deploy/postgres/entrypoint.sh').read_text().replace('exec /usr/local/bin/docker-entrypoint.sh "$@"', 'exec "$@"')
            result = subprocess.run(['sh', '-c', wrapper, 'fixture', str(probe)], env={**os.environ, 'PORCHLIGHT_BACKUP_PASSWORD_FILE': str(path)}, capture_output=True)
            self.assertEqual(result.returncode, 0, 'role entrypoint did not read 0600 file before dropping UID')
            self.assertFalse(result.stdout)

    def test_dp06_daily_live_renewal(self):
        scheduler = (ROOT / 'server/api/src/scheduler.rs').read_text()
        self.assertIn('server_certificate', scheduler, 'DP-06: no daily certificate job')
        self.assertIn('ResolvesServerCert for', (ROOT / 'server/api/src/tls.rs').read_text())

    def test_dp07_detection_and_shutdown(self):
        for name in ('deploy/compose.yml', 'deploy/compose.funnel.yml'):
            _, entries = services(name)
            for service in ('api', 'postgres', 'tailscale'):
                if service not in entries: continue
                self.assertRegex(entries[service], r'retries: [234]\n', f'DP-07: {service} steady failure detection')
        self.assertIn('stop_grace_period: 60s', services('deploy/compose.yml')[1]['postgres'])

    def test_dp08_prune_is_explicit_and_scoped(self):
        self.assertTrue(hasattr(ops, 'prune_plan'), 'DP-08: no conservative prune planner')
        self.assertIn('disk_headroom', (ROOT / 'server/api/src/scheduler.rs').read_text())
        with tempfile.TemporaryDirectory(dir=ROOT / 'var') as tmp:
            root = Path(tmp)
            for version in ('0.1.4', '0.1.5', '0.1.6', '0.1.7'):
                release = root / 'releases' / version
                release.mkdir(parents=True)
                inventory = {name: {} for name in ('install.sh', 'compose.yml', 'porchlight-api-' + version + '.tar', 'porchlight-backup-' + version + '.tar')}
                (release / 'manifest.json').write_text(json.dumps({'version': version, 'format_version': 1, 'files': inventory}))
            (root / '.env').write_text('PORCHLIGHT_VERSION=0.1.7\n')
            (root / 'release-current').symlink_to('releases/0.1.7')
            (root / 'release-previous').symlink_to('releases/0.1.6')
            pinned = root / 'backups/rollback/porchlight-fixture.tar.age'
            pinned.parent.mkdir(parents=True)
            pinned.touch()
            # Unknown encrypted recovery points fail closed, not guessed by date.
            with self.assertRaises(ValueError): ops.prune_plan(root, [], [])
            pinned.with_suffix(pinned.suffix + '.version').write_text('0.1.5\n')
            dirs, images = ops.prune_plan(root, ['porchlight-api:0.1.4', 'porchlight-api:0.1.5', 'unrelated:0.1.4', 'porchlight-api:dev'], [])
            self.assertEqual(dirs, [root / 'releases/0.1.4'])
            self.assertEqual(images, ['porchlight-api:0.1.4'])
            self.assertEqual(ops.prune_plan(root, ['porchlight-api:0.1.4'], ['porchlight-api:0.1.4']), ([], []))
            inventory = ['porchlight-api:0.1.4\nunrelated:0.1.4\nporchlight-api:dev\n', '']
            with patch('subprocess.check_output', side_effect=inventory), patch('subprocess.run') as remove, contextlib.redirect_stdout(io.StringIO()) as output:
                ops.prune(root)
                remove.assert_not_called()
                self.assertIn('WOULD REMOVE image porchlight-api:0.1.4', output.getvalue())
                self.assertNotIn('unrelated', output.getvalue())
                self.assertTrue((root / 'releases/0.1.4').exists())
            with patch('subprocess.check_output', side_effect=inventory), patch('subprocess.run') as remove, patch.object(ops.shutil, 'rmtree') as rmtree, contextlib.redirect_stdout(io.StringIO()):
                ops.prune(root, apply=True)
                remove.assert_called_once_with(['docker', 'image', 'rm', 'porchlight-api:0.1.4'], check=True)
                rmtree.assert_called_once_with(root / 'releases/0.1.4')
            # A similarly named unrelated application directory is never enough authority to delete it.
            (root / 'releases/0.1.4/manifest.json').write_text('{"version":"0.1.4"}')
            with self.assertRaises(ValueError): ops.prune_plan(root, [], [])
            (root / '.env').write_text('PORCHLIGHT_VERSION=0.1.6\n')
            with self.assertRaises(ValueError): ops.prune_plan(root, [], [])

    def test_dp09_observability(self):
        _, entries = services('deploy/compose.yml')
        for flag in ('shared_preload_libraries=pg_stat_statements', 'pg_stat_statements.track=top', 'effective_cache_size=1GB', 'autovacuum_work_mem=64MB'):
            self.assertIn(flag, entries['postgres'], 'DP-09: ' + flag)
        bootstrap = (ROOT / 'deploy/postgres/bootstrap-roles.sql').read_text()
        self.assertIn('CREATE EXTENSION IF NOT EXISTS pg_stat_statements', bootstrap)
        self.assertIn('REVOKE ALL ON FUNCTION public.pg_stat_statements_reset', bootstrap)


if __name__ == '__main__':
    unittest.main()
