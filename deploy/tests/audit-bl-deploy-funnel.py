#!/usr/bin/env python3
"""Backlog Funnel contracts; actual embedded probe with deterministic sockets/clock."""
import ast
import io
from pathlib import Path
import socket
import ssl
import subprocess
import sys
import types
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SOURCE = (ROOT / 'scripts/deploy-server.sh').read_text()
PROBE = SOURCE.split("probe=$(cat <<'PY_PROBE'\n", 1)[1].split('\nPY_PROBE', 1)[0]


class Probe(unittest.TestCase):
    def run_probe(self, outcomes, connect_cost=0):
        clock = [0]
        budgets = []
        sockets = []
        class Connection:
            def __init__(self, *args): self.timeout = None; sockets.append(self)
            def settimeout(self, value): self.timeout = value
            def connect(self, address): clock[0] += connect_cost
            def close(self): pass
            def __enter__(self): return self
            def __exit__(self, *args): self.close()
            def version(self): return 'TLSv1.3'
            def recv(self, count):
                value = outcomes.pop(0)
                if isinstance(value, Exception): raise value
                return value
        class Context:
            def wrap_socket(self, raw, **kwargs): budgets.append(raw.timeout); return raw
        err = ssl.SSLError('certificate required')
        err.reason = 'TLSV13_ALERT_CERTIFICATE_REQUIRED'
        outcomes[:] = [err if v == 'required' else v for v in outcomes]
        with patch.object(socket, 'getaddrinfo', return_value=[(socket.AF_INET, socket.SOCK_STREAM, 6, '', ('192.168.77.50', 443))]), \
                patch.object(socket, 'socket', Connection), patch.object(ssl, 'create_default_context', return_value=Context()), \
                patch('time.monotonic', side_effect=lambda: clock[0]), patch.object(sys, 'argv', ['probe', 'quiet-node.example.ts.net', 'fixture-ca']), \
                patch.object(sys, 'stdout', io.StringIO()) as output, patch.object(sys, 'stderr', io.StringIO()):
            with self.assertRaises(SystemExit) as result:
                exec(compile(PROBE, '<production funnel probe>', 'exec'), {})
        return result.exception.code, output.getvalue(), budgets, len(sockets)

    def test_reset_retries_fresh_connection_but_requires_specific_alert(self):
        code, output, _, count = self.run_probe([ConnectionResetError(), 'required'])
        self.assertEqual(code, 0, 'transient reset must retry a fresh connection')
        self.assertIn('PASS public-tls', output)
        self.assertEqual(count, 2)
        code, output, _, count = self.run_probe([ConnectionResetError()] * 3)
        self.assertNotEqual(code, 0)
        self.assertNotIn('PASS public-tls', output)
        self.assertEqual(count, 3)
        for terminal in [b'', b'data', ssl.SSLError('wrong alert')]:
            code, output, _, count = self.run_probe([terminal])
            self.assertNotEqual(code, 0)
            self.assertNotIn('PASS public-tls', output)
            self.assertEqual(count, 1)

    def test_handshake_uses_remaining_budget_after_connect(self):
        code, _, budgets, _ = self.run_probe(['required'], connect_cost=7)
        self.assertEqual(code, 0)
        self.assertEqual(budgets, [13], 'connect must consume handshake budget')

    def test_cleanup_suppresses_signals_before_any_local_assignments(self):
        source = (ROOT / 'deploy/install.sh').read_text()
        block = source.split('cleanup_install() {\n', 1)[1]
        first = next(line.strip() for line in block.splitlines() if line.strip() and not line.strip().startswith('#'))
        self.assertEqual(first, "trap '' INT TERM HUP PIPE")
        self.assertIn("trap 'cleanup_install \"$?\"' EXIT", source)

    def test_watchdog_reports_dns_phase_separately(self):
        self.assertIn('Funnel DNS watchdog expired', SOURCE)
        self.assertIn('Funnel TLS watchdog expired', SOURCE)


# Reuse the established offline installer fixture, without running its old tests.
namespace = {'__name__': 'funnel_fixture'}
inline = (ROOT / 'deploy/tests/funnel.sh').read_text().split("<<'PY'\n", 1)[1].split('\nPY\n', 1)[0]
with patch.object(sys, 'argv', ['fixture', str(ROOT)]):
    exec(compile(inline.split('unittest.main(', 1)[0], '<funnel fixture>', 'exec'), namespace)
Base = namespace['InstallerTests']


class Installer(unittest.TestCase):
    setUp = Base.setUp
    run_install = Base.run_install
    funnel = Base.funnel
    records = Base.records

    def test_configurable_api_health_deadline(self):
        result = self.funnel('--ts-authkey-file', str(self.key), '--api-health-timeout', '75')
        self.assertEqual(result.returncode, 0)
        api = [r['argv'] for r in self.records() if 'up' in r['argv'] and r['argv'][-1] == 'api']
        self.assertTrue(api)
        self.assertTrue(all('--wait-timeout' in a and a[a.index('--wait-timeout') + 1] == '75' for a in api))
        for value in ('0', '601', '-1', 'not-a-number'):
            self.funnel('--no-authkey', '--api-health-timeout', value, ok=False)

    def test_failed_activation_retains_only_private_sanitized_diagnostic(self):
        self.funnel('--ts-authkey-file', str(self.key), env={'FAIL_START': '1'}, ok=False)
        saved = list((self.stack / 'tmp').glob('failure.*/summary.txt'))
        self.assertEqual(len(saved), 1, 'failure diagnostic must survive removal of install stage')
        self.assertEqual(saved[0].stat().st_mode & 0o777, 0o600)
        self.assertEqual(saved[0].parent.stat().st_mode & 0o777, 0o700)
        text = saved[0].read_text()
        self.assertIn('mode=funnel', text)
        self.assertIn('status=1', text)
        self.assertNotIn(self.key.read_text().strip(), text)


def load_tests(loader, tests, pattern):
    return unittest.TestSuite(loader.loadTestsFromTestCase(cls) for cls in (Probe, Installer))


if __name__ == '__main__': unittest.main()
