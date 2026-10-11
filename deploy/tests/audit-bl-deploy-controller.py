#!/usr/bin/env python3
"""Execute generated remote programs locally; no network, Docker or real credentials."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]


class Controller(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='bl-controller.', dir=ROOT / 'var')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'scripts').mkdir()
        (self.root / 'bin').mkdir()
        (self.root / 'var').mkdir()
        self.stack = self.root / 'remote/porchlight'
        (self.stack / 'tmp').mkdir(parents=True)
        (self.stack / 'release-current').mkdir()
        shutil.copyfile(ROOT / 'scripts/deploy-server.sh', self.root / 'scripts/deploy-server.sh')
        (self.root / 'scripts/local-env.sh').write_text('porchlight_local_env() { :; }\n')
        (self.stack / 'release-current/install.sh').write_text("#!/bin/bash\nprintf 'PASS funnel quiet-node.example.ts.net\\n'\n")
        ssh = self.root / 'bin/ssh'
        ssh.write_text('''#!/usr/bin/python3 -I
import os,shlex,subprocess,sys
args=shlex.split(sys.argv[-1])
data=sys.stdin.buffer.read()
if os.environ.get('SHORT_STREAM'): data=data[:len(data)//2]
sys.exit(subprocess.run(args,input=data).returncode)
''')
        ssh.chmod(0o755)
        self.key = self.root / 'var/authkey'
        self.key.write_bytes(b'private-fixture-bytes-not-a-real-credential')
        self.key.chmod(0o600)
        self.env = dict(os.environ, PATH=str(self.root / 'bin') + ':/usr/bin:/bin',
                        PORCHLIGHT_DEPLOY_HOST='offline-test-host',
                        PORCHLIGHT_DEPLOY_DATA_MOUNT=str(self.root / 'remote'), PORCHLIGHT_DEPLOY_DRY_RUN='0')

    def run_controller(self, **env):
        result = subprocess.run(['bash', str(self.root / 'scripts/deploy-server.sh'), 'funnel',
                                 '--ts-hostname', 'quiet-node', '--tailscale-image', 'tailscale/tailscale@sha256:' + 'a' * 64,
                                 '--ts-authkey-file', str(self.key)], env=dict(self.env, **env),
                                capture_output=True, text=True, timeout=15)
        self.assertNotIn(self.key.read_text(), result.stdout + result.stderr)
        return result

    def test_truncated_successful_transport_is_not_published(self):
        result = self.run_controller(SHORT_STREAM='1')
        self.assertNotEqual(result.returncode, 0, 'nonempty truncated upload must fail integrity validation')
        self.assertNotIn('PASS funnel ', result.stdout)
        self.assertFalse((self.root / 'var/m9/funnel-fqdn').exists())
        self.assertFalse((self.stack / 'tmp/m9-ts-authkey').exists())

    def test_only_owned_private_old_regular_stages_are_swept(self):
        directory = self.stack / 'tmp'
        old = directory / '.m9-ts-authkey.old'
        young = directory / '.m9-ts-authkey.young'
        public = directory / '.m9-ts-authkey.public'
        link = directory / '.m9-ts-authkey.link'
        unrelated = directory / 'unrelated'
        for path in (old, young, public, unrelated):
            path.write_text('fixture')
            path.chmod(0o600)
        public.chmod(0o644)
        link.symlink_to(unrelated)
        for path in (old, public, unrelated): os.utime(path, (time.time() - 90000,) * 2)
        result = self.run_controller()
        self.assertEqual(result.returncode, 0)
        self.assertFalse(old.exists(), 'stale interrupted private upload was not swept')
        self.assertTrue(all(p.exists() for p in (young, public, link, unrelated)))

    def test_success_is_not_printed_before_local_save(self):
        (self.root / 'var/m9/funnel-fqdn').mkdir(parents=True)
        result = self.run_controller()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('PASS funnel ', result.stdout, 'final PASS preceded failed local publication')

    def test_remote_failure_never_relays_a_final_pass(self):
        installer = self.stack / 'release-current/install.sh'
        installer.write_text(installer.read_text() + 'exit 1\n')
        result = self.run_controller()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('PASS funnel ', result.stdout)
        self.assertFalse((self.root / 'var/m9/funnel-fqdn').exists())

    def test_success_publishes_private_fqdn(self):
        result = self.run_controller()
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout.count('PASS funnel '), 1)
        saved = self.root / 'var/m9/funnel-fqdn'
        self.assertEqual(saved.read_text(), 'quiet-node.example.ts.net\n')
        self.assertEqual(saved.stat().st_mode & 0o777, 0o600)


if __name__ == '__main__': unittest.main()
