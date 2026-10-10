#!/usr/bin/env python3
"""Offline LAN helper tests: fake ip, no Docker, sockets or network traffic."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DevLanTest(unittest.TestCase):
    def setUp(self):
        (ROOT / 'var/tests').mkdir(parents=True, exist_ok=True)
        self.work = tempfile.TemporaryDirectory(prefix='dev-lan.', dir=ROOT / 'var/tests')
        self.addCleanup(self.work.cleanup)
        fake = Path(self.work.name) / 'ip'
        fake.write_text('''#!/bin/bash
printf '%s\\n' "$*" >> "$FAKE_IP_CALLS"
case "$*" in
  '-4 -o route show table main default') printf '%s\\n' "${FAKE_DEFAULTS:-}" ;;
  '-4 -o route get 1.1.1.1') printf '%s\\n' "$FAKE_ROUTE"; exit "${FAKE_ROUTE_RC:-0}" ;;
  '-4 -o addr show scope global') printf '%s\\n' "$FAKE_ADDRS" ;;
  *) exit 99 ;;
esac
''')
        fake.chmod(0o755)
        self.calls = Path(self.work.name) / 'calls'
        self.sysfs = Path(self.work.name) / 'sys'
        for interface in ('eth0', 'wlan0'):
            (self.sysfs / 'class/net' / interface / 'device').mkdir(parents=True)
        self.env = {
            'DEV_LAN_SYSFS_ROOT': str(self.sysfs),
            'PATH': self.work.name + ':/usr/bin:/bin',
            'HOME': self.work.name,
            'FAKE_IP_CALLS': str(self.calls),
            'FAKE_ROUTE': '1.1.1.1 via 192.0.2.1 dev wlan0 src 192.0.2.42 uid 1000\n    cache',
            'FAKE_ADDRS': '2: wlan0 inet 192.0.2.42/24 brd 192.0.2.255 scope global wlan0\n'
                          '3: eth0 inet 198.51.100.5/25 brd 198.51.100.127 scope global eth0',
        }

    def run_helper(self, **overrides):
        return subprocess.run(['bash', '-eu', '-c',
                               '. "$1"; dev_lan_config; printf "%s\\n" "$DEV_LAN_IP" "$DEV_LAN_CIDR" "$ENROLL_CIDRS" "$PORCHLIGHT_ENDPOINTS"',
                               'fixture', str(ROOT / 'scripts/dev-lan.sh')],
                              env=dict(self.env, **overrides), capture_output=True, text=True)

    def test_route_source_matches_address_and_canonical_network(self):
        result = self.run_helper()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), [
            '192.0.2.42', '192.0.2.0/24', '192.0.2.0/24,172.31.250.1/32,172.31.251.1/32',
            'https://localhost:8443,https://10.0.2.2:8443,https://192.0.2.42:8443'])
        self.assertEqual(self.calls.read_text().splitlines(), [
            '-4 -o route show table main default', '-4 -o route get 1.1.1.1',
            '-4 -o addr show scope global'])

    def test_laptop_physical_default_wins_over_vpn_and_bridges(self):
        result = self.run_helper(
            FAKE_DEFAULTS='default via 192.168.77.1 dev eth0 proto dhcp src 192.168.77.29 metric 100\n'
                          'default via 192.168.77.1 dev wlan0 proto dhcp src 192.168.77.93 metric 600',
            FAKE_ROUTE='1.1.1.1 dev wg0 src 10.99.0.2 uid 1000\n    cache',
            FAKE_ADDRS='2: eth0 inet 192.168.77.29/24 brd 192.168.77.255 scope global eth0\n'
                       '3: wlan0 inet 192.168.77.93/24 brd 192.168.77.255 scope global wlan0\n'
                       '4: docker0 inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0\n'
                       '5: br-backend inet 172.18.0.1/16 brd 172.18.255.255 scope global br-backend\n'
                       '6: br-dev-backend inet 172.31.250.1/24 brd 172.31.250.255 scope global br-dev-backend\n'
                       '7: br-dev-ingress inet 172.31.251.1/24 brd 172.31.251.255 scope global br-dev-ingress\n'
                       '8: wg0 inet 10.99.0.2/32 scope global wg0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[:2], ['192.168.77.29', '192.168.77.0/24'])
        self.assertNotIn('route get', self.calls.read_text())

    def test_wifi_only_uses_physical_default_without_route_get(self):
        result = self.run_helper(FAKE_DEFAULTS='default via 192.0.2.1 dev wlan0 src 192.0.2.42 metric 600',
                                 FAKE_ROUTE_RC='1')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[:2], ['192.0.2.42', '192.0.2.0/24'])
        self.assertNotIn('route get', self.calls.read_text())

    def test_metric_missing_zero_and_ties_first_listed(self):
        eth = 'default dev eth0 src 198.51.100.5'
        wifi = 'default dev wlan0 src 192.0.2.42'
        for routes, expected in [
                (wifi + ' metric 600\n' + eth + ' metric 100', '198.51.100.5'),
                (wifi + ' metric 1\n' + eth, '198.51.100.5'),
                (wifi + '\n' + eth + ' metric 0', '192.0.2.42'),
                (eth + ' metric 100\n' + wifi + ' metric 100', '198.51.100.5')]:
            with self.subTest(routes=routes):
                result = self.run_helper(FAKE_DEFAULTS=routes, FAKE_ROUTE_RC='1')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.splitlines()[0], expected)

    def test_virtual_defaults_are_ignored_even_with_lower_metrics(self):
        result = self.run_helper(
            FAKE_DEFAULTS='default dev wg0 src 10.99.0.2\n'
                          'default dev docker0 src 172.17.0.1\n'
                          'default dev br-backend src 172.18.0.1\n'
                          'default dev wlan0 src 192.0.2.42 metric 600', FAKE_ROUTE_RC='1')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[0], '192.0.2.42')
        self.assertNotIn('route get', self.calls.read_text())

    def test_no_src_uses_primary_global_address_of_selected_interface(self):
        result = self.run_helper(
            FAKE_DEFAULTS='default dev eth0 metric 100', FAKE_ROUTE_RC='1',
            FAKE_ADDRS='3: wlan0 inet 192.0.2.42/24 scope global wlan0\n'
                       '2: eth0 inet 198.51.100.6/25 scope global secondary eth0\n'
                       '2: eth0 inet 198.51.100.5/25 scope global eth0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[:2], ['198.51.100.5', '198.51.100.0/25'])
        self.assertNotIn('route get', self.calls.read_text())

    def test_subnet_matches_selected_interface_not_duplicate_ip_elsewhere(self):
        result = self.run_helper(
            FAKE_DEFAULTS='default dev wlan0 src 192.0.2.42',
            FAKE_ADDRS=self.env['FAKE_ADDRS'] + '\n4: docker0 inet 192.0.2.42/16 scope global docker0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[:2], ['192.0.2.42', '192.0.2.0/24'])

    def test_physical_default_without_global_address_errors_without_fallback(self):
        result = self.run_helper(FAKE_DEFAULTS='default dev eth0 metric 100', FAKE_ADDRS='')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('set DEV_LAN_IP and DEV_LAN_CIDR', result.stderr)
        self.assertNotIn('route get', self.calls.read_text())

    def test_vpn_only_fallback_rejects_nonphysical_host_prefix(self):
        for overrides in ({}, {'DEV_LAN_CIDR': '10.0.0.0/8'}):
            with self.subTest(overrides=overrides):
                result = self.run_helper(
                    FAKE_DEFAULTS='default dev wg0 src 10.99.0.2',
                    FAKE_ROUTE='1.1.1.1 dev wg0 src 10.99.0.2',
                    FAKE_ADDRS='8: wg0 inet 10.99.0.2/32 scope global wg0',
                    **overrides)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('tunnel-like', result.stderr)
                self.assertIn('set DEV_LAN_IP and DEV_LAN_CIDR', result.stderr)
                self.assertEqual(result.stdout, '')
        self.assertIn('route get', self.calls.read_text())

    def test_nonphysical_non_host_prefix_fallback_remains_supported(self):
        result = self.run_helper(
            FAKE_ROUTE='1.1.1.1 dev br-lan src 192.0.2.42',
            FAKE_ADDRS='8: br-lan inet 192.0.2.42/24 scope global br-lan')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[:2], ['192.0.2.42', '192.0.2.0/24'])

    def test_source_only_has_no_ip_calls_or_output(self):
        result = subprocess.run(['bash', '-eu', '-c', '. "$1"', 'fixture', str(ROOT / 'scripts/dev-lan.sh')],
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout + result.stderr, '')
        self.assertFalse(self.calls.exists())

    def test_full_overrides_skip_ip_and_canonicalize_cidr(self):
        result = self.run_helper(DEV_LAN_IP='198.51.100.5', DEV_LAN_CIDR='198.51.100.5/25')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[:2], ['198.51.100.5', '198.51.100.0/25'])
        self.assertFalse(self.calls.exists())

    def test_ip_override_finds_its_own_subnet_not_the_default_route(self):
        result = self.run_helper(DEV_LAN_IP='198.51.100.5')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[:2], ['198.51.100.5', '198.51.100.0/25'])
        self.assertNotIn('route', self.calls.read_text())

    def test_cidr_override_and_custom_gateways_and_port(self):
        result = self.run_helper(DEV_LAN_CIDR='192.0.2.0/25', BACKEND_GATEWAY='203.0.113.1',
                                 INGRESS_GATEWAY='203.0.113.2', LAN_PORT='18443')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('192.0.2.0/25,203.0.113.1/32,203.0.113.2/32', result.stdout)
        self.assertIn('https://192.0.2.42:18443', result.stdout)
        self.assertNotIn('addr', self.calls.read_text())

    def test_existing_enrollment_and_endpoints_overrides_are_preserved(self):
        result = self.run_helper(ENROLL_CIDRS='192.0.2.0/25', PORCHLIGHT_ENDPOINTS='https://example.test:8443')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[2:], ['192.0.2.0/25', 'https://example.test:8443'])

    def test_missing_route_or_source_or_subnet_fails_clearly(self):
        for overrides, message in [({'FAKE_ROUTE_RC': '1'}, 'DEV_LAN_IP'),
                                   ({'FAKE_ROUTE': '1.1.1.1 dev wlan0'}, 'DEV_LAN_IP'),
                                   ({'FAKE_ADDRS': ''}, 'DEV_LAN_CIDR'),
                                   ({'FAKE_ADDRS': self.env['FAKE_ADDRS'] + '\n2: wlan0 inet 192.0.2.42/25 scope global wlan0'}, 'DEV_LAN_CIDR')]:
            with self.subTest(overrides=overrides):
                result = self.run_helper(**overrides)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('dev-lan:', result.stderr)
                self.assertIn(message, result.stderr)
                self.assertEqual(result.stdout, '')

    def test_invalid_or_mismatched_overrides_fail_without_evaluation(self):
        for address, cidr in [('not-ip', '192.0.2.0/24'), ('127.0.0.1', '127.0.0.0/8'),
                              ('192.0.2.42', '198.51.100.0/24'), ('192.0.2.42', '192.0.2.0'),
                              ('$(touch should-not-exist)', '192.0.2.0/24')]:
            with self.subTest(address=address):
                result = self.run_helper(DEV_LAN_IP=address, DEV_LAN_CIDR=cidr)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('dev-lan:', result.stderr)
                self.assertEqual(result.stdout, '')


if __name__ == '__main__':
    unittest.main()
