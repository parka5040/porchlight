#!/usr/bin/env python3
"""Offline desktop extraction space and installed-image retention checks."""
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
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('desktop_operations', ROOT / 'deploy/lib/operations.py')
operations = importlib.util.module_from_spec(spec)
spec.loader.exec_module(operations)


class DesktopUpdateTests(unittest.TestCase):
    def setUp(self):
        parent = ROOT / 'var/tests'
        parent.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix='desktop-retention-', dir=parent)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_retention_keeps_current_previous_and_a_running_image(self):
        releases = self.root / 'releases'
        for version in ('0.1.0', '0.2.0', '0.3.0', '0.4.0'):
            image = releases / version
            (image / 'app/bin').mkdir(parents=True)
            (image / 'install.sh').write_text('fixture')
            (image / 'release-manifest.json').write_text(json.dumps({'version': version}))
        (self.root / 'current').symlink_to('releases/0.4.0')
        (self.root / 'previous').symlink_to('releases/0.3.0')
        running = releases / '0.1.0/app/bin/running'
        shutil.copy2(shutil.which('sleep'), running)
        process = subprocess.Popen([str(running), '60'])
        try:
            self.prune()
            self.assertEqual({'0.1.0', '0.3.0', '0.4.0'}, {p.name for p in releases.iterdir()})
        finally:
            process.terminate()
            process.wait(timeout=3)
        self.prune()
        self.assertEqual({'0.3.0', '0.4.0'}, {p.name for p in releases.iterdir()})

    def prune(self):
        subprocess.run(['bash', '-c', '. "$1"; desktop_prune_images "$2"', 'desktop-test',
                        str(ROOT / 'deploy/lib/operations.sh'), str(self.root)], check=True)

    def test_extraction_refuses_low_space_before_writing_image(self):
        archive = self.root / 'image.tar'
        with tarfile.open(archive, 'w') as tar:
            header = tarfile.TarInfo('porchlight/file')
            header.size = 4
            tar.addfile(header, io.BytesIO(b'app!'))
        destination = self.root / 'extracted'
        with patch.object(operations.shutil, 'disk_usage', return_value=shutil._ntuple_diskusage(10, 9, 1)):
            with self.assertRaises(OSError):
                operations.extract(archive, destination, '--desktop')
        self.assertFalse(destination.exists())


if __name__ == '__main__':
    unittest.main()
