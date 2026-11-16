#!/usr/bin/env python3
"""Desktop activation and retention share one installed-state transaction lock."""
import fcntl
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class DesktopLockTests(unittest.TestCase):
    def setUp(self):
        parent = ROOT / 'var/tests'
        parent.mkdir(parents=True, exist_ok=True)
        temp = tempfile.TemporaryDirectory(prefix='desktop-lock-', dir=parent)
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.base = self.root / 'data/porchlight'
        for version in ('0.1.0', '0.2.0', '0.3.0'):
            image = self.base / 'releases' / version
            (image / 'app').mkdir(parents=True)
            (image / 'install.sh').write_text('fixture')
            (image / 'release-manifest.json').write_text(json.dumps({'version': version}))
        (self.base / 'current').symlink_to('releases/0.3.0')
        (self.base / 'previous').symlink_to('releases/0.2.0')
        self.installer = self.root / 'install.sh'
        subprocess.run(['python3', '-I', str(ROOT / 'scripts/build-installer.py'), str(ROOT), str(self.installer), 'TEST-PUBLIC-KEY'], check=True, capture_output=True)
        self.env = dict(os.environ, HOME=str(self.root), XDG_DATA_HOME=str(self.root / 'data'), XDG_STATE_HOME=str(self.root / 'state'))
        self.env.pop('PORCHLIGHT_INSTALL_LOCK_ROOT', None)

    def test_rollback_cannot_change_snapshot_while_prune_owns_installed_state(self):
        with (self.base / '.installer.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            # Pause the pruning transaction after its keep-set snapshot. An activation must not enter.
            keep = {os.readlink(self.base / name) for name in ('current', 'previous')}
            result = subprocess.run(['bash', str(self.installer), 'desktop', '--rollback'], env=self.env, capture_output=True, text=True, timeout=10)
            self.assertEqual(2, result.returncode, 'Concurrent rollback entered the prune transaction')
            self.assertIn('installer already running', result.stderr)
            self.assertEqual(keep, {os.readlink(self.base / name) for name in ('current', 'previous')})
        result = subprocess.run(['bash', str(self.installer), 'desktop', '--rollback'], env=self.env, capture_output=True, text=True, timeout=10)
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertTrue((self.base / 'current').resolve(strict=True).is_dir())
        self.assertTrue((self.base / 'previous').resolve(strict=True).is_dir())

    def test_standalone_prune_cannot_delete_during_activation(self):
        with (self.base / '.installer.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            result = subprocess.run(['bash', '-c', '. "$1"; desktop_prune_images "$2"', 'desktop-test', str(ROOT / 'deploy/lib/operations.sh'), str(self.base)], env=self.env, capture_output=True, text=True, timeout=10)
            self.assertNotEqual(0, result.returncode, 'Pruning ignored the activation lock')
            self.assertTrue((self.base / 'releases/0.1.0').is_dir())


if __name__ == '__main__':
    unittest.main()
