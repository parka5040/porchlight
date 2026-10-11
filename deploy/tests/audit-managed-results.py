#!/usr/bin/env python3
"""Offline contract for the controller's fail-closed managed-device runner."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
GUARD = ROOT / "scripts/check-managed-results.py"
PASS = '<testsuite tests="1" failures="0" errors="0" skipped="0"><testcase name="smoke" /></testsuite>'
ZERO = '<testsuites tests="0" failures="0" errors="0" skipped="0" />'


class AuditManagedResultsTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="audit-managed-", dir=ROOT / "var")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.results = self.root / "results"
        for device in ("api35", "api36"):
            self.result(device, PASS)

    def result(self, device, xml, code="0"):
        directory = self.results / device
        directory.mkdir(parents=True, exist_ok=True)
        (directory / f"TEST-{device}.xml").write_text(xml)
        (directory / "test-result-exit-code.txt").write_text(code)

    def guard(self):
        return subprocess.run(["python3", "-I", str(GUARD), str(self.results), "api35", "api36"],
                              capture_output=True, text=True)

    def test_accepts_both_devices_with_tests(self):
        result = self.guard()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_accepts_nested_suites_and_multiple_reports(self):
        self.result("api35", '<testsuites tests="2" failures="0" errors="0">' + PASS + PASS + '</testsuites>')
        (self.results / "api36/TEST-extra.xml").write_text(PASS)
        result = self.guard()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_rejects_missing_exit_code(self):
        (self.results / "api36/test-result-exit-code.txt").unlink()
        self.assertNotEqual(self.guard().returncode, 0, "Partial run omitted the UTP completion code")

    def test_rejects_zero_on_either_device(self):
        for device in ("api35", "api36"):
            with self.subTest(device=device):
                self.result(device, ZERO)
                result = self.guard()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("zero tests", result.stderr)
                self.result(device, PASS)

    def test_rejects_missing_malformed_failed_crashed_or_all_skipped(self):
        bad = ["", "<testsuites", '<testsuite tests="1" errors="1"><error>crashed</error></testsuite>',
               '<testsuite tests="1" failures="1"><testcase><failure /></testcase></testsuite>',
               '<testsuite tests="1" skipped="1"><testcase><skipped /></testcase></testsuite>',
               '<testsuite tests="1"><testcase><error /></testcase></testsuite>',
               '<testsuite tests="1" />', '<testsuite><testcase /></testsuite>',
               '<testsuite tests="not-a-number"><testcase /></testsuite>',
               '<testsuites tests="1">' + PASS + ZERO + '</testsuites>']
        for xml in bad:
            with self.subTest(xml=xml):
                self.result("api36", xml)
                self.assertNotEqual(self.guard().returncode, 0)
        self.result("api36", PASS, code="1")
        self.assertNotEqual(self.guard().returncode, 0)
        shutil.rmtree(self.results / "api36")
        self.assertNotEqual(self.guard().returncode, 0)

    def wrapper(self, debug=PASS, minified=PASS, exit_code=0, stale=False, args=()):
        scripts = self.root / "scripts"
        scripts.mkdir()
        shutil.copyfile(ROOT / "scripts/test-managed-clients.sh", scripts / "test-managed-clients.sh")
        if GUARD.exists():
            shutil.copyfile(GUARD, scripts / GUARD.name)
        (scripts / "env.sh").write_text("")
        (scripts / "emulator-common.sh").write_text("reap_stale_netsimd() { :; }\n")
        client = self.root / "client"
        client.mkdir()
        gradlew = client / "gradlew"
        gradlew.write_text("#!/usr/bin/env python3\nfrom pathlib import Path\nimport sys\n"
                          "variant = 'minified' if '-Pporchlight.minifiedSmoke=true' in sys.argv else 'debug'\n"
                          "with Path('calls').open('a') as f: f.write(variant + '\\n')\n"
                          f"xml = {{'debug': {debug!r}, 'minified': {minified!r}}}[variant]\n"
                          "if xml is not None:\n"
                          "    for device in ('api35', 'api36'):\n"
                          "        dest = Path('androidApp/build/outputs/androidTest-results/managedDevice') / variant / device\n"
                          "        dest.mkdir(parents=True, exist_ok=True)\n"
                          "        (dest / ('TEST-' + device + '.xml')).write_text(xml)\n"
                          "        (dest / 'test-result-exit-code.txt').write_text('0')\n"
                          f"sys.exit({exit_code})\n")
        gradlew.chmod(0o755)
        if stale:
            for variant in ("debug", "minified"):
                dest = client / "androidApp/build/outputs/androidTest-results/managedDevice" / variant
                shutil.copytree(self.results, dest)
        return subprocess.run(["bash", str(scripts / "test-managed-clients.sh"), *args],
                              capture_output=True, text=True)

    def test_wrapper_rejects_silent_zero_debug(self):
        result = self.wrapper(debug=ZERO)
        self.assertNotEqual(result.returncode, 0, "fix1: wrapper accepted Gradle SUCCESS with tests=0")
        self.assertEqual((self.root / "client/calls").read_text(), "debug\n")

    def test_wrapper_rejects_silent_zero_minified(self):
        result = self.wrapper(minified=ZERO)
        self.assertNotEqual(result.returncode, 0, "fix1: minified Gradle SUCCESS hid tests=0")

    def test_wrapper_cannot_reuse_stale_success_after_crash(self):
        result = self.wrapper(debug=None, stale=True)
        self.assertNotEqual(result.returncode, 0, "fix1: stale XML hid missing results")

    def test_wrapper_propagates_gradle_failure(self):
        result = self.wrapper(exit_code=17)
        self.assertEqual(result.returncode, 17)

    def test_wrapper_accepts_both_variants(self):
        result = self.wrapper()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "client/calls").read_text(), "debug\nminified\n")

    def test_minified_only_still_checks_results(self):
        result = self.wrapper(minified=ZERO, args=("--minified",))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.root / "client/calls").read_text(), "minified\n")

    def test_full_ci_uses_guarded_wrapper(self):
        ci = (ROOT / "scripts/ci.sh").read_text()
        self.assertIn('"$root/scripts/test-managed-clients.sh"', ci)
        self.assertNotIn("phonesGroup", ci)


if __name__ == "__main__":
    unittest.main()
