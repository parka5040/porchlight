#!/usr/bin/env python3
"""Bootstrap regression: trusted installer, unchanged signed inventory, hostile cwd.
Signature tooling is mocked (as in release.sh); no Docker or owner keys needed.
"""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]

def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value

class Tests(unittest.TestCase):
    def test_unsigned_cwd_module_never_runs_before_inventory_rejection(self):
        (ROOT / 'var/tests').mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / 'var/tests') as tmp:
            p = Path(tmp)
            source = p / 'source'
            source.mkdir()
            (source / 'artifact').write_text('signed payload')
            ops = module('ops', ROOT / 'deploy/lib/operations.py')
            ops.manifest(source, '0.1.0')
            # Signature stand-in accepts only the exact manifest bytes signed above.
            (source / 'manifest.json.minisig').write_text(ops.digest(source / 'manifest.json')['sha256'])
            binpath = p / 'bin'
            binpath.mkdir()
            verifier = binpath / 'minisign'
            verifier.write_text('''#!/bin/bash
set -eu
while (($#)); do case $1 in -Vm) m=$2; shift 2;; -x) s=$2; shift 2;; -P) shift 2;; *) exit 2;; esac; done
[[ $(sha256sum "$m" | cut -d' ' -f1) == $(< "$s") ]]
''')
            verifier.chmod(0o755)
            marker = p / 'unsigned-code-executed'
            (source / 'json.py').write_text(f'from pathlib import Path\nPath({str(marker)!r}).touch()\nraise RuntimeError("unsigned shadow module executed")\n')
            # Optional read-only baseline input; fixtures always stay in this copy's var/.
            installer_root = Path(os.environ.get('PORCHLIGHT_TEST_INSTALLER_ROOT', ROOT))
            builder = module('builder', installer_root / 'scripts/build-installer.py')
            installer = p / 'trusted-install.sh'
            installer.write_text(builder.build(installer_root, 'fixture-pinned-key'))
            env = dict(os.environ, PATH=str(binpath) + ':' + os.environ['PATH'], HOME=str(p / 'home'),
                       XDG_STATE_HOME=str(p / 'state'), XDG_DATA_HOME=str(p / 'data'))
            result = subprocess.run(['bash', str(installer), 'desktop', '--source', '.'],
                                    cwd=source, env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0, 'unsigned inventory addition accepted')
            self.assertFalse(marker.exists(), 'unsigned json.py executed before inventory verification')
            self.assertIn(b'operations: validation or I/O failed', result.stderr)

if __name__ == '__main__':
    unittest.main()
