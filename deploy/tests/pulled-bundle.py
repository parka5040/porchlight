#!/usr/bin/env python3
"""Private staging checks, no Docker or keys required."""
import hashlib
import importlib.util
import json
import pathlib
import tempfile
import unittest
ROOT = pathlib.Path(__file__).resolve().parents[2]

class Staging(unittest.TestCase):
    def test_cold_copy_is_private_and_reverified(self):
        spec = importlib.util.spec_from_file_location('stage_pulled', ROOT / 'deploy/scripts/stage-pulled-bundle.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory(dir=ROOT / 'var') as tmp:
            base = pathlib.Path(tmp)
            source = base / 'home/.local/share/porchlight/backups'
            source.mkdir(parents=True)
            bundle = source / 'bundle.tar.age'; bundle.write_bytes(b'encrypted fixture')
            receipt = source / 'bundle.json'
            metadata = dict(size=bundle.stat().st_size, sha256=hashlib.sha256(bundle.read_bytes()).hexdigest())
            receipt.write_text(json.dumps(metadata))
            target = base / 'private'; target.mkdir(mode=0o700)
            staged = module.stage(bundle, receipt, target)
            self.assertEqual(staged.read_bytes(), bundle.read_bytes())
            self.assertEqual(staged.stat().st_mode & 0o777, 0o600)
            self.assertEqual((target / 'receipt.json').stat().st_mode & 0o777, 0o600)
            self.assertEqual(bundle.read_bytes(), b'encrypted fixture')
            for invalid in [dict(metadata, size=1), dict(metadata, sha256='0'*64)]:
                receipt.write_text(json.dumps(invalid))
                with tempfile.TemporaryDirectory(dir=base) as out:
                    with self.assertRaises(ValueError): module.stage(bundle, receipt, pathlib.Path(out))
            link = source / 'link'; link.symlink_to(bundle)
            with tempfile.TemporaryDirectory(dir=base) as out:
                with self.assertRaises(ValueError): module.stage(link, receipt, pathlib.Path(out))

if __name__ == '__main__': unittest.main()
