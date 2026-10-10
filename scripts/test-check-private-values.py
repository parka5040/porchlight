#!/usr/bin/env python3
"""Contract for check-private-values.py: boundaries, LAN prefix, skipped paths, and values never printed."""
import pathlib
import subprocess
import sys
import tempfile
import unittest

CHECK = pathlib.Path(__file__).resolve().with_name('check-private-values.py')
PROFILE = ('PORCHLIGHT_DEPLOY_HOST=fixture-host\nPORCHLIGHT_DEPLOY_DATA_MOUNT=/srv/fixture-mount\n'
           'PORCHLIGHT_DEPLOY_LAN=10.20.30.40/24\nPORCHLIGHT_PRIVATE_STRINGS=Pat Example, pexample ,ab\n'
           'PORCHLIGHT_RELEASE_REPO=example-owner/porchlight\n')
VALUES = ('fixture-host', '/srv/fixture-mount', '10.20.30.', 'Pat Example', 'pexample', '/home/fixtureuser',
          'example-owner')


class CheckPrivateValues(unittest.TestCase):
    def tree(self, files, profile=PROFILE):
        root = pathlib.Path(self.enterContext(tempfile.TemporaryDirectory()))
        if profile is not None:
            (root / '.env').write_text(profile)
        for name, text in files.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text(text)
        return root

    def run_check(self, root):
        return subprocess.run([sys.executable, '-I', str(CHECK), '--root', str(root), '--home', '/home/fixtureuser'],
                              capture_output=True, text=True)

    def test_near_misses_pass(self):
        root = self.tree({'clean.txt': 'fixture-hostname pexample5040 10.20.300.1 110.20.30.4 /srv/fixture-mounted '
                                       '/home/fixtureuser2 ab Patrick Examples example-owner2 xexample-owner porchlight\n'})
        result = self.run_check(root)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('PASS no private values in 1 committed files (8 checks)', result.stdout)

    def test_release_owner_is_private_but_not_repository_name(self):
        root = self.tree({'docs/release.md': 'repository porchlight\nhttps://github.com/EXAMPLE-OWNER/porchlight\n'})
        result = self.run_check(root)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('FAIL private value (PORCHLIGHT_RELEASE_REPO) in docs/release.md:2', result.stdout)
        self.assertIn('1 private value(s)', result.stdout)
        self.assertNotIn('example-owner', (result.stdout + result.stderr).lower())
        root = self.tree({'clean.txt': 'ab/porchlight\n'}, profile='PORCHLIGHT_RELEASE_REPO=ab/porchlight\n')
        self.assertEqual(self.run_check(root).returncode, 0)

    def test_each_value_fails_by_key_and_line_without_printing_it(self):
        root = self.tree({'docs/run.md': 'intro\nssh FIXTURE-HOST\n/srv/fixture-mount/porchlight\nhost 10.20.30.99\n',
                          'src/a.kt': '// by Pat Example (pexample)\nval p = "/home/fixtureuser/x"\n'})
        result = self.run_check(root)
        self.assertEqual(result.returncode, 1, result.stdout)
        for expected in ['(PORCHLIGHT_DEPLOY_HOST) in docs/run.md:2', '(PORCHLIGHT_DEPLOY_DATA_MOUNT) in docs/run.md:3',
                         '(PORCHLIGHT_DEPLOY_LAN) in docs/run.md:4', '(PORCHLIGHT_PRIVATE_STRINGS) in src/a.kt:1',
                         '(home directory) in src/a.kt:2']:
            self.assertIn('FAIL private value ' + expected, result.stdout)
        self.assertIn('6 private value(s)', result.stdout)
        for value in VALUES:
            self.assertNotIn(value.lower(), result.stdout.lower())

    def test_untracked_paths_are_skipped(self):
        root = self.tree({name: 'fixture-host\n' for name in [
            'var/x.txt', 'dist/0.1.0/x', 'docs/plans/p.md', 'docs/operations/gates/m1.log', 'CLAUDE.md',
            'server/CLAUDE.md', 'server/target/x', 'client/build/x', '.env.local', '.git/config']})
        self.assertEqual(self.run_check(root).returncode, 0)
        (root / '.env.prod.example').write_text('fixture-host\n')
        self.assertEqual(self.run_check(root).returncode, 1)

    def test_without_profile_only_home_is_checked(self):
        root = self.tree({'a.txt': 'fixture-host 10.20.30.40\n'}, profile=None)
        result = self.run_check(root)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('no .env, home directory only', result.stdout)
        (root / 'b.txt').write_text('/home/fixtureuser/.config\n')
        self.assertEqual(self.run_check(root).returncode, 1)


if __name__ == '__main__':
    unittest.main()
