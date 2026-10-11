#!/usr/bin/env python3
"""Offline resource/footprint contracts and crash recovery fault injection."""
import contextlib
import fcntl
import importlib.util
import io
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('backup', ROOT / 'deploy/backup/backup.py')
backup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(backup)


def services(name):
    text = (ROOT / name).read_text()
    body = text.split('services:\n', 1)[1].split('\nnetworks:', 1)[0]
    return text, dict(re.findall(r'^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)', body, re.M | re.S))


class Contracts(unittest.TestCase):
    def test_o1_01_every_service_has_bounded_logs(self):
        for name in ('deploy/compose.yml', 'deploy/compose.funnel.yml'):
            text, entries = services(name)
            self.assertIn('x-logging: &bounded-logging\n', text, 'O1-01: missing shared logging policy')
            self.assertRegex(text, r'driver: json-file\n\s+options:\n\s+max-size: "10m"\n\s+max-file: "3"')
            for service, body in entries.items():
                self.assertIn('    logging: *bounded-logging\n', body, service)

    def test_o1_01_daemon_defense_in_depth(self):
        text = (ROOT / 'deploy/nixos-porchlight.nix').read_text()
        self.assertIn('virtualisation.docker.daemon.settings', text, 'O1-01: no daemon rotation')
        for item in ('virtualisation.docker.logDriver = "json-file";', '"max-size" = "10m";', '"max-file" = "3";'):
            self.assertIn(item, text)

    def test_o1_01_no_per_peer_info_logging(self):
        text = (ROOT / 'server/api/src/server.rs').read_text()
        check = text.split('fn check_peer(', 1)[1].split('\nasync fn tcp(', 1)[0]
        self.assertNotIn('tracing::info!', check, 'O1-01: per-connection INFO log')

    def test_o1_02_resource_and_connection_budget(self):
        for name in ('deploy/compose.yml', 'deploy/compose.funnel.yml'):
            _, entries = services(name)
            for service, body in entries.items():
                for field in ('mem_limit', 'cpus', 'pids_limit'):
                    self.assertTrue(re.search(rf'^    {field}: ["\d]', body, re.M),
                                    f'O1-02: {service} missing {field}')
        _, entries = services('deploy/compose.yml')
        for setting in ('shared_buffers=512MB', 'work_mem=4MB', 'maintenance_work_mem=128MB', 'max_connections=50'):
            self.assertIn(setting, entries['postgres'], setting)
        config = (ROOT / 'server/api/src/config.rs').read_text()
        self.assertRegex(config, r'default_value_t = 16\)\]\s+pub db_pool_size')
        listeners = sum((ROOT / f'server/api/src/{name}.rs').read_text().count('crate::db::listener_pool(')
                        for name in ('projector', 'derive_worker', 'stream_hub', 'revocation'))
        self.assertEqual(listeners, 4, 'recalculate connection headroom')
        self.assertEqual((ROOT / 'server/api/src/scheduler.rs').read_text().count('.max_connections(1)'), 3)

    def test_o1_03_steady_health_probes_have_fast_startup(self):
        for name in ('deploy/compose.yml', 'deploy/compose.funnel.yml'):
            _, entries = services(name)
            for service, body in entries.items():
                if 'healthcheck:' in body:
                    self.assertIn('      interval: 30s\n', body, f'O1-03: {service} steady probe')
                    self.assertRegex(body, r'      start_interval: [25]s\n')
                    self.assertRegex(body, r'      start_period: (60|120)s\n')

    def test_f1_03_client_only_image(self):
        text = (ROOT / 'deploy/backup/Dockerfile').read_text()
        stages = re.findall(r'^FROM (.+)$', text, re.M)
        self.assertRegex(stages[-1], r'^debian:trixie-slim@sha256:([a-f0-9]{64}|CONTROLLER_PIN)$',
                         'F1-03: runtime still includes PostgreSQL server')
        self.assertRegex(stages[0], r'^postgres:18\.6@sha256:[a-f0-9]{64} AS client-tools$')
        for name in ('age', 'python3', 'tzdata', 'bash', 'openssl'):
            self.assertIn(name, text)
        export = (ROOT / 'deploy/backup/client-tools.sh').read_text()
        self.assertIn('pg_dump psql pg_restore', export)
        self.assertIn('ldd', export)
        self.assertIn('gconv', export)
        self.assertIn('LC_MESSAGES', export)
        self.assertIn('18.6', export)
        self.assertIn('! command -v postgres', text)

    def test_f1_08_client_artifacts_built_once(self):
        text = (ROOT / 'scripts/ci.sh').read_text()
        for task in (':androidApp:assembleDebug', ':desktopApp:jar'):
            self.assertEqual(len(re.findall(re.escape(task) + r'(?=\s)', text)), 1,
                             f'F1-08: duplicate {task}')
        self.assertIn(':shared:lintAndroidMain', text)

    def test_f1_09_worker_caps(self):
        for name in ('scripts/c1b-desktop-e2e.sh', 'scripts/c1b-emulator-e2e.sh', 'deploy/scripts/m3-demo.sh'):
            for line in (ROOT / name).read_text().splitlines():
                if './gradlew ' in line:
                    self.assertIn('--max-workers=4', line, f'F1-09: uncapped {name}')
        self.assertIn('org.gradle.workers.max=4\n', (ROOT / 'client/gradle.properties').read_text())


class Recovery(unittest.TestCase):
    def test_o1_04_reclaim_before_failed_sql_and_retry(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'var', prefix='audit-ops-') as tmp:
            base = Path(tmp)
            backups = base / 'backups'
            backups.mkdir()
            published = backups / 'porchlight-fixture.tar.age'
            published.write_text('published')
            with patch.dict(os.environ, TMPDIR=str(base / 'work'), PORCHLIGHT_DUMP_TMPDIR=str(base / 'disk')):
                stale = [backup.work_root(backups) / 'work-abandoned',
                         backup.work_root(backups, disk=True) / 'work-dump', backups / '.work-old']
                for path in stale:
                    path.mkdir()
                    (path / 'dump').write_text('disposable')
                partial = backups / '.partial-abandoned'
                partial.write_text('partial')
                stale.append(partial)
                with open(backups / '.backup.lock', 'a') as lock, patch.object(backup, 'sql', side_effect=[RuntimeError('fixture write failure'), '']) as sql:
                    fcntl.flock(lock, fcntl.LOCK_EX)
                    with self.assertRaises(RuntimeError):
                        backup.recover_locked(backups)
                    self.assertTrue(all(not path.exists() for path in stale), 'O1-04: SQL failure retained abandoned work')
                    self.assertEqual(published.read_text(), 'published')
                    backup.recover_locked(backups)
                    self.assertEqual(sql.call_count, 2)
                    self.assertTrue(all("WHERE status='running'" in call.args[0] for call in sql.call_args_list))

    def test_o1_04_daemon_retries_repair_before_new_backup(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'var', prefix='audit-ops-') as tmp:
            sleeps = []
            def sleep(_):
                sleeps.append(1)
                if len(sleeps) > 60:
                    raise backup.BackupInterrupted()
            with patch.dict(os.environ, PORCHLIGHT_BACKUP_DIR=tmp), patch.object(backup, 'heartbeat'), \
                    patch.object(backup, 'recover_locked', side_effect=[RuntimeError('fixture write failure'), None]) as recover, \
                    patch.object(backup, 'sql', return_value='{}') as sql, patch.object(backup.ops, 'backup_due', return_value=False), \
                    patch.object(backup, 'run') as run, patch.object(backup.time, 'sleep', side_effect=sleep), \
                    contextlib.redirect_stderr(io.StringIO()):
                backup.daemon()
                self.assertEqual(recover.call_count, 2)
                self.assertEqual(sql.call_count, 1)
                run.assert_not_called()


class Discard(unittest.TestCase):
    def test_f1_07_explicit_locked_discard(self):
        text = (ROOT / 'deploy/scripts/update-gate-build.sh').read_text()
        harness = 'set -euo pipefail\nroot=$1; discard=1\n' + text[text.index('fail() {'):text.index('copy=$root/')]
        # The real pre-build/lock/discard block, with no Docker access or build tools.
        harness = 'docker() { printf "%s\\n" "${MOCK_CONTAINERS:-}"; }\n' + harness
        with tempfile.TemporaryDirectory(dir=ROOT / 'var', prefix='audit-ops-') as tmp:
            base = Path(tmp)
            state = base / 'var/update-gate'
            state.mkdir(parents=True)
            for name in ('dists', 'android'):
                (state / name).mkdir()
                (state / name / 'fixture').write_text('payload')
            (state / 'build.json').write_text('{}')
            (state / 'diagnostic.log').write_text('retain')
            with open(state / '.lock', 'a') as lock:
                fcntl.flock(lock, fcntl.LOCK_EX)
                result = subprocess.run(['bash', '-c', harness, 'fixture', tmp], capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertTrue((state / 'dists').exists())
            result = subprocess.run(['bash', '-c', harness, 'fixture', tmp], env={**os.environ, 'MOCK_CONTAINERS': 'porchlight-update-gate-fixture'}, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0, 'F1-07: discard must refuse surviving gate containers')
            self.assertTrue((state / 'dists').exists())
            result = subprocess.run(['bash', '-c', harness, 'fixture', tmp], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((state / 'dists').exists(), 'F1-07: gate dists have no explicit discard')
            self.assertFalse((state / 'android').exists())
            self.assertFalse((state / 'build.json').exists())
            self.assertEqual((state / 'diagnostic.log').read_text(), 'retain')
            self.assertTrue((state / '.lock').exists())


if __name__ == '__main__':
    unittest.main()
