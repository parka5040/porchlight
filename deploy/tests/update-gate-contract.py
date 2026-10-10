#!/usr/bin/env python3
"""Offline safety/shape checks and driver unit tests, never a live gate attestation."""
import ast
import hashlib
import importlib.util
import json
import pathlib
import re
import subprocess
import sys
import tarfile
import tempfile
import unittest

sys.dont_write_bytecode = True  # -I intentionally ignores PYTHONDONTWRITEBYTECODE in the environment.
ROOT = pathlib.Path(__file__).resolve().parents[2]
DRIVER = ROOT / 'deploy/scripts/update-gate.py'


def load_driver():
    spec = importlib.util.spec_from_file_location('update_gate', DRIVER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Contract(unittest.TestCase):
    def test_shell_safety_and_hooks(self):
        scripts = {}
        for name in ('update-gate-build.sh', 'update-acceptance.sh', 'update-signer-fixture.sh'):
            path = ROOT / 'deploy/scripts' / name
            self.assertTrue(path.is_file(), name)
            subprocess.run(['bash', '-n', str(path)], check=True)
            text = scripts[name] = path.read_text()
            self.assertIn('set -euo pipefail', text)
            self.assertIn('trap cleanup EXIT', text)
            self.assertNotIn('set -x', text)
            self.assertNotIn('deploy-server.sh', text)
            self.assertNotIn('PORCHLIGHT_DEPLOY_', text)
            self.assertNotIn('m8-', text)
            for index, snippet in enumerate(re.findall(r"<<'PY'\n(.*?)\nPY", text, re.S)):
                compile(snippet, f'{name}:{index}', 'exec')
            for command in re.findall(r'\badb\s+[^\n]+', text):
                self.assertTrue(command.startswith('adb -s "$serial" '), command)
            # Writes to the real dist/key directories must never be introduced here.
            self.assertNotRegex(text, r'\$(?:root|\{root\})/(?:dist|var/release)(?:/|\b)')
        build = scripts['update-gate-build.sh']
        for hook in ('scripts/wt.sh create updgate-build', 'scripts/wt.sh drop updgate-build',
                     '[[ $root != */var/wt/* ]]', 'var/update-gate', 'var/wt/updgate-build',
                     'scripts/release-keys.sh', '-importkeystore', '-srcstorepass:file', '-deststorepass:file',
                     '-destalias porchlight', 'debug.keystore', 'android-signer.sha256', 'minisign.pub',
                     '9.9.8 9.9.9 9.9.10', 'scripts/release.sh', ':androidApp:assembleDebugAndroidTest',
                     '--offline --max-workers=4', 'record-build', 'PASS update gate build'):
            self.assertIn(hook, build)
        gate = scripts['update-acceptance.sh']
        for hook in ('verify-build', 'trap cleanup EXIT', 'dc down --volumes', 'porchlight-update-gate-',
                     'python3 -I -m http.server', '--bind 127.0.0.1', 'mirror', 'latest 9.9.10',
                     '--no-funnel', '--no-backup', 'age-keygen', 'cli dev-init', 'probe:desktop',
                     'cli items add', 'cli items show', 'healthcheck', 'assert-clean-upgrade',
                     'PORCHLIGHT_RELEASE_BASE_URL=', 'PORCHLIGHT_INSTALL_TESTING=1',
                     'porchlight-upgrade" 9.9.9', 'porchlight-upgrade" 9.9.8',
                     'ROLLED BACK to 9.9.9', 'XDG_DATA_HOME=', 'XDG_CONFIG_HOME=', 'XDG_STATE_HOME=',
                     'desktop-stage', 'tamper', '--rollback', 'porch-api35', '-port 5582',
                     '. scripts/emulator-common.sh', 'reap_stale_netsimd', 'REQUEST_INSTALL_PACKAGES allow',
                     'UpdateGateTest', 'UpdateSignerInstrumentedTest', 'android-confirm',
                     'uninstall app.porchlight', 'PASS update acceptance'):
            self.assertIn(hook, gate)
        self.assertIn("fail 'shut down existing emulators first'", gate)
        self.assertIn("fail 'shut down starting emulators first'", gate)
        self.assertIn('if [[ -n $emulator_pid ]]', gate)
        self.assertNotIn('reap_stale_netsimd()', gate)

    def test_driver_and_instrumentation_hooks(self):
        text = DRIVER.read_text()
        tree = ast.parse(text)
        for node in ast.walk(tree):
            if isinstance(node, ast.List) and node.elts and isinstance(node.elts[0], ast.Constant) and node.elts[0].value == 'adb':
                self.assertEqual(node.elts[1].value, '-s')
        for hook in ('/v1/client-release', '0.0.1', '426', 'ssl.create_default_context',
                     'load_cert_chain', 'manifest.json.minisig', '--mtime=@0', '--verbatim-files-from',
                     'uiautomator', 'input', 'tap', 'versionName=9.9.9', '120', 'run-as app.porchlight',
                     'umask 077; cat > files/update-gate.json', 'forced migration failure'):
            self.assertIn(hook, text)
        tests = ROOT / 'client/androidApp/src/androidTest/kotlin/app/porchlight/android'
        gate = (tests / 'UpdateGateTest.kt').read_text()
        signer = (tests / 'UpdateSignerInstrumentedTest.kt').read_text()
        for hook in ('@ControllerOnly', 'UPDATE_GATE_INPUT', 'input.delete()', 'controller.enroll',
                     'UpdateState.Available', 'updates.update()', 'isCommitted', 'dumpWindowHierarchy'):
            self.assertIn(hook, gate)
        for hook in ('GET_SIGNING_CERTIFICATES', 'getPackageArchiveInfo', 'apkPreflight', 'apkDecision',
                     'DIFFERENT_APK_KEY', 'sourceDir', 'wrong-signer.apk', '[0-9a-f]{64}'):
            self.assertIn(hook, signer)
        self.assertNotIn('@ControllerOnly', signer)
        gradle = (ROOT / 'client/androidApp/build.gradle.kts').read_text()
        self.assertIn('update-signer-fixture.sh', gradle)
        self.assertIn('generateUpdateSignerFixture', gradle)
        self.assertIn('update-gate-contract.py', (ROOT / 'scripts/ci.sh').read_text())


class Driver(unittest.TestCase):
    def setUp(self):
        self.driver = load_driver()
        scratch = ROOT / 'var/tests'
        scratch.mkdir(parents=True, exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(prefix='update-gate-contract.', dir=scratch)
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)

    def test_bump_only_workspace_lock_entries_and_failure_migration(self):
        root = self.root
        (root / 'server/db/migrations').mkdir(parents=True)
        (root / 'VERSION').write_text('0.1.6\n')
        (root / 'server/Cargo.toml').write_text('[workspace]\nmembers=["core"]\n[workspace.package]\nversion = "0.1.6"\n')
        (root / 'server/Cargo.lock').write_text('version = 4\n\n[[package]]\nname = "porchlight-core"\nversion = "0.1.6"\n\n[[package]]\nname = "other"\nversion = "0.1.6"\n')
        (root / 'server/db/migrations/0001_base.sql').write_text('SELECT 1;\n')
        self.driver.bump(root, '9.9.10')
        self.assertEqual((root / 'VERSION').read_text(), '9.9.10\n')
        self.assertIn('name = "porchlight-core"\nversion = "9.9.10"', (root / 'server/Cargo.lock').read_text())
        self.assertIn('name = "other"\nversion = "0.1.6"', (root / 'server/Cargo.lock').read_text())
        sql = list((root / 'server/db/migrations').glob('*update_gate*.sql'))
        self.assertEqual(len(sql), 1)
        self.assertIn('RAISE EXCEPTION', sql[0].read_text())
        self.assertEqual((root / 'server/db/migrations/0001_base.sql').read_text(), 'SELECT 1;\n')

    def test_path_guards(self):
        for bad in ('../outside', '/absolute', 'lib/../escape', 'lib//x', '-option', 'x\\y', 'lib/__pycache__/x.pyc'):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                self.driver.member_path(self.root, bad)
        self.assertEqual(self.driver.member_path(self.root, 'lib/operations.py'), self.root / 'lib/operations.py')
        (self.root / 'linked').symlink_to(self.root.parent)
        with self.assertRaises(ValueError):
            self.driver.member_path(self.root, 'linked/outside')

    def test_manifest_files_detect_corruption(self):
        (self.root / 'payload').write_bytes(b'fixture')
        record = {'size': 7, 'sha256': hashlib.sha256(b'fixture').hexdigest()}
        self.driver.check_file(self.root / 'payload', record)
        (self.root / 'payload').write_bytes(b'Fixture')
        with self.assertRaises(ValueError):
            self.driver.check_file(self.root / 'payload', record)

    def test_mirror_matches_publish_tar_format_and_latest_switch(self):
        for version in self.driver.VERSIONS:
            source = self.root / 'dists' / version
            (source / 'lib').mkdir(parents=True)
            files = ['install.sh', 'lib/operations.py', f'porchlight-{version}.apk']
            for name in files:
                (source / name).write_bytes(b'fixture\n')
            (source / 'install.sh').chmod(0o700)
            manifest = {'version': version, 'files': {name: self.driver.digest(source / name) for name in files}}
            (source / 'manifest.json').write_text(json.dumps(manifest))
            (source / 'manifest.json.minisig').write_text('fixture signature')
        first, second = self.root / 'mirror-one', self.root / 'mirror-two'
        self.driver.mirror(self.root, first)
        self.driver.mirror(self.root, second)
        for version in self.driver.VERSIONS:
            relative = f'download/v{version}/porchlight-{version}-dist.tar'
            self.assertEqual((first / relative).read_bytes(), (second / relative).read_bytes())
            with tarfile.open(first / relative) as archive:
                members = archive.getmembers()
                expected = sorted(['install.sh', 'lib/operations.py', f'porchlight-{version}.apk', 'manifest.json', 'manifest.json.minisig'])
                self.assertEqual([m.name for m in members], expected)
                self.assertTrue(all(m.isfile() and m.uid == m.gid == m.mtime == 0 for m in members))
        self.assertEqual(json.loads((first / 'latest/download/manifest.json').read_text())['version'], '9.9.9')
        self.driver.latest(first, '9.9.10')
        self.assertEqual(json.loads((first / 'latest/download/manifest.json').read_text())['version'], '9.9.10')

    def test_cleanup_assertions_reject_payloads_and_plain_dumps(self):
        stack = self.root
        (stack / 'incoming').mkdir()
        (stack / 'rollback').mkdir()
        self.driver.assert_clean_upgrade(stack)
        (stack / 'incoming/payload').write_text('retained')
        with self.assertRaises(ValueError):
            self.driver.assert_clean_upgrade(stack)
        self.driver.assert_clean_upgrade(stack, incoming=False)  # failed release retained after rollback
        (stack / 'rollback/pre-upgrade-9.9.9.dump').write_text('plain fixture')
        with self.assertRaises(ValueError):
            self.driver.assert_clean_upgrade(stack, incoming=False)

    def test_confirmation_uses_unique_system_button_bounds(self):
        xml = '<hierarchy><node package="com.android.packageinstaller" resource-id="android:id/button1" text="Update" enabled="true" clickable="true" bounds="[10,20][90,60]"/></hierarchy>'
        self.assertEqual(self.driver.confirm_button(xml), (50, 40))
        for bad in (xml.replace('com.android.packageinstaller', 'app.porchlight'), xml.replace('Update', 'Cancel'),
                    xml.replace('enabled="true"', 'enabled="false"'), '<hierarchy/>', xml.replace('</hierarchy>', xml + '</hierarchy>')):
            self.assertIsNone(self.driver.confirm_button(bad))


if __name__ == '__main__':
    unittest.main(verbosity=2)
