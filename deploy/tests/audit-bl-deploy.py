#!/usr/bin/env python3
"""Offline backlog regressions: bounded phases, trusted helpers and archive ends."""
import ast
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


backup = load('backup_bl', 'deploy/backup/backup.py')
verify = load('export_bl', 'deploy/lib/verify-export.py')


class Backlog(unittest.TestCase):
    def test_restore_helper_uses_explicit_trusted_path(self):
        source = (ROOT / 'deploy/scripts/restore-drill.sh').read_text()
        self.assertIn('"$drill" "$root/deploy/lib/blob_integrity.py"', source)
        self.assertIn("spec_from_file_location('blob_integrity',sys.argv[2])", source)
        self.assertNotIn('Path.cwd()', source)

    def test_prehash_updates_only_one_digest(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'var') as work:
            path = Path(work) / 'blob'
            data = b'content' * 1000
            path.write_bytes(data)
            real = hashlib.sha256
            with patch.object(backup.hashlib, 'sha256', wraps=real) as hashes, patch.object(backup, 'heartbeat'):
                self.assertEqual(backup.external_digest(path), {'sha256': real(data).hexdigest(), 'size': len(data)})
                self.assertEqual(hashes.call_count, 1, 'prehash must not compute the same digest twice')

    def test_blob_limit_matches_schema(self):
        migration = (ROOT / 'server/db/migrations/0036_attachments.sql').read_text()
        limit = int(re.search(r'size BETWEEN 0 AND (\d+)', migration)[1])
        row = {'household_id': 'household', 'sha256': 'a' * 64, 'size': limit}
        backup.blob_integrity.metadata([row], [])
        with self.assertRaises(ValueError):
            backup.blob_integrity.metadata([dict(row, size=limit + 1)], [])

    def test_slow_predump_phases_are_bounded_not_fake_progress(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'var') as work, \
                patch.dict(os.environ, PORCHLIGHT_BACKUP_DIR=work), \
                patch.object(backup.time, 'monotonic', return_value=1000) as clock, \
                patch.object(backup, 'sql', return_value='1'):
            heartbeat = Path(work) / '.heartbeat'
            heartbeat.touch()
            os.utime(heartbeat, (0, 0))
            with backup.bounded_phase():
                clock.return_value = 1200  # A legitimate query/copy longer than the idle watchdog.
                backup.health()
                self.assertEqual(heartbeat.stat().st_mtime, 0, 'waiting is not progress')
                with backup.bounded_phase():
                    clock.return_value = 1601
                    with self.assertRaises(AssertionError): backup.health()
            self.assertFalse((Path(work) / '.phase').exists())
            os.utime(heartbeat, (0, 0))
            with self.assertRaises(AssertionError): backup.health()

    def test_blocking_predump_operations_enter_phase(self):
        tree = ast.parse((ROOT / 'deploy/backup/backup.py').read_text())
        for name in ('command', 'copy_private', 'query'):
            functions = [n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef) and n.name == name]
            self.assertTrue(any(isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id == 'bounded_phase'
                                for f in functions for n in ast.walk(f)), name + ' lacks bounded phase')

    def test_slow_snapshot_read_uses_phase_without_heartbeat_pulses(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'var') as work, \
                patch.dict(os.environ, PORCHLIGHT_BACKUP_DIR=work), \
                patch.object(backup.time, 'monotonic', return_value=1000) as clock, \
                patch.object(backup, 'sql', return_value='1'):
            heartbeat = Path(work) / '.heartbeat'
            heartbeat.touch(); os.utime(heartbeat, (0, 0))
            class Pipe(io.StringIO):
                def readline(inner):
                    clock.return_value = 1200
                    backup.health()
                    self.assertEqual(heartbeat.stat().st_mtime, 0)
                    clock.return_value = 1601
                    with self.assertRaises(AssertionError): backup.health()
                    return 'snapshot-row\n'
            snapshot = backup.Snapshot()
            snapshot.p = type('Process', (), {'stdin': io.StringIO(), 'stdout': Pipe()})()
            self.assertEqual(snapshot.query('fixture query'), 'snapshot-row')
            self.assertFalse((Path(work) / '.phase').exists())

    def test_tar_requires_both_exact_zero_blocks(self):
        out = io.BytesIO()
        with tarfile.open(fileobj=out, mode='w') as archive:
            for name, obj in [('data.json', {'format_version': 2, 'tables': {}}),
                              ('complete.json', {'format_version': 2, 'complete': True, 'table_rows': {}, 'blob_count': 0, 'blob_sha256': []})]:
                raw = json.dumps(obj).encode()
                info = tarfile.TarInfo(name); info.size = len(raw)
                archive.addfile(info, io.BytesIO(raw))
            end = archive.offset
        content = out.getvalue()[:end]
        with tempfile.TemporaryDirectory(dir=ROOT / 'var') as work:
            path = Path(work) / 'export.gz'
            path.write_bytes(gzip.compress(content + bytes(1024)))
            self.assertEqual(verify.verify(path), {})
            for suffix in (b'', bytes(512), bytes(1023), b'x' + bytes(1023), bytes(512) + b'x' + bytes(511)):
                path.write_bytes(gzip.compress(content + suffix))
                with self.assertRaises(ValueError): verify.verify(path)

    def test_stream_diagnostics_survive_failure_only(self):
        source = (ROOT / 'deploy/scripts/m5-demo-stream.sh').read_text()
        self.assertNotIn('private diagnostics discarded', source)
        self.assertIn('if rc == 0:', source)
        self.assertIn('private diagnostics retained', source)

    def test_android_viewer_launch_is_main_dispatched(self):
        source = (ROOT / 'client/shared/src/androidMain/kotlin/app/porchlight/shared/screens/collaboration/AttachmentPlatform.android.kt').read_text()
        self.assertTrue('override suspend fun launch() = withContext(Dispatchers.Main.immediate)' in source,
                        'Android viewer launch lacks explicit Main dispatch')

    def test_desktop_hint_only_for_first_install(self):
        source = (ROOT / 'deploy/install.sh').read_text().split('install_desktop() {', 1)[1]
        hint = source.index("printf 'Device key:")
        self.assertLess(source.index('if ((rollback))'), hint)
        self.assertIn('if [[ ! -L $base/current ]]; then', source[:hint])


if __name__ == '__main__':
    unittest.main()
