#!/usr/bin/env python3
"""Offline regressions for status propagation and literal installer diagnostics."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


def shell(program, *args):
    return subprocess.run(
        ["bash", "-eu", "-c", program, "audit-shellcheck", *args],
        cwd=ROOT, text=True, capture_output=True,
    )


class ShellcheckRegressionTest(unittest.TestCase):
    def dev_exports(self, failing):
        # Exercise the real exports without Docker or the tree-copy startup guard.
        script = (ROOT / "scripts/dev-db.sh").read_text()
        exports = script.split("set -a\n", 1)[1].split("# Postgres creates", 1)[0]
        program = '''
root=$PWD
envfile=/dev/null
id() { if [[ $1 == "$failure" ]]; then return 23; fi; command id "$@"; }
tr() { if [[ $failure == tr ]]; then return 23; fi; command tr "$@"; }
failure=$1
''' + exports + "\nprintf 'continued after exports\\n'\n"
        result = shell(program, failing)
        self.assertEqual(result.returncode, 23, "failed substitution was masked by export")
        self.assertEqual(result.stdout, "", "continued after a failed substitution")

    def test_uid_failure_aborts(self):
        self.dev_exports("-u")

    def test_gid_failure_aborts(self):
        self.dev_exports("-g")

    def test_version_failure_aborts(self):
        self.dev_exports("tr")

    def test_diagnostics_are_literal_text(self):
        # The wrapper must accept text, not a printf format, even for % and backslashes.
        script = (ROOT / "deploy/install.sh").read_text()
        helper = next(line for line in script.splitlines() if line.startswith("say()"))
        message = r"Recovery logs: /srv/data/100%/quote' %s %q \n"
        result = shell('exec {install_stderr}>&2\n' + helper + '\nsay "$1"\n', message)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stderr, message + "\n")

    def test_timeout_validation_boundaries(self):
        script = (ROOT / "deploy/install.sh").read_text()
        validation = script.split("done\n", 1)[1].split("if ((funnel));", 1)[0]
        for value in ("1", "30", "600", "0", "601", "999", "1000", "01", "-1", "", "invalid"):
            with self.subTest(value=value):
                result = shell('api_health_timeout=$1\n' + validation, value)
                valid = value in ("1", "30", "600")
                self.assertEqual(result.returncode, 0 if valid else 2)
                self.assertEqual(result.stderr, "" if valid else "API health timeout must be 1-600 seconds\n")

    def test_dead_diagnostic_channel_is_nonfatal(self):
        script = (ROOT / "deploy/install.sh").read_text()
        helper = next(line for line in script.splitlines() if line.startswith("say()"))
        result = shell('exec {install_stderr}>/dev/full\n' + helper + '\nsay "$1"\n', "recovery")
        self.assertEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
