#!/usr/bin/env python3
"""Host tests of the actual controller driver; no adb/bus/network, no sleeps."""
import importlib.util
import io
from contextlib import redirect_stdout
from unittest.mock import patch
import json
from pathlib import Path
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

# -I ignores PYTHONDONTWRITEBYTECODE: protect standalone isolated imports as well.
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('m5_client_gate', Path(__file__).resolve().parents[1] / 'scripts/m5-client-gate.py')
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class DriverTest(unittest.TestCase):
    def test_incremental_android_inventory_and_original_dismissal_key(self):
        row = {'key': 'n', 'version': 2, 'posted': True, 'actionKey': 'original'}
        xml = '<map><string name="table">' + json.dumps([row]) + '</string></map>'
        self.assertEqual([row], gate.preference_inventory(ET.fromstring(xml), 'delivery'))
        self.assertEqual('original', gate.posted_action_key({'delivery': [row]}, 'n'))
        self.assertRaises(ValueError, gate.posted_action_key, {'delivery': [dict(row, actionKey=None)]}, 'n')
        self.assertRaises(ValueError, gate.posted_action_key, {'delivery': [dict(row, posted=False)]}, 'n')
        self.assertRaises(ValueError, gate.posted_action_key, {'delivery': [dict(row, dismissed=True)]}, 'n')
        alarm = {'key': 'r', 'code': 4, 'generation': 1, 'fireAt': '2035-01-01T00:00:00Z'}
        incremental = ET.Element('map')
        ET.SubElement(incremental, 'int', {'name': 'nextCode', 'value': '5'})
        ET.SubElement(incremental, 'string', {'name': 'ticket:r'}).text = json.dumps(alarm)
        self.assertEqual([alarm], gate.preference_inventory(incremental, 'alarms'))

    def test_exact_denial_only_on_api31_32(self):
        for api, expected in ((31, 'denied'), (32, 'denied'), (35, 'granted'), (36, 'granted')):
            driver = object.__new__(gate.Gate)
            driver.log = io.BytesIO()
            calls = []
            driver.shell = lambda *args: str(api) if args == ('getprop', 'ro.build.version.sdk') else ''
            def adb(*args, **kwargs):
                calls.append(args)
                return b'OK (2 tests)'
            driver.adb = adb
            driver.instrument('C4PlatformControllerTest', 'denied')
            values = calls[0]
            index = values.index('C4_EXACT_EXPECTED')
            self.assertEqual(expected, values[index + 1])
            self.assertEqual(api >= 33, b'denied scenario overridden to granted' in driver.log.getvalue())

    def test_platform_runs_declare_the_notification_grant_they_set(self):
        # Each C4PlatformControllerTest run is told the POST_NOTIFICATIONS state set just before it (the test asserts it).
        driver = object.__new__(gate.Gate)
        driver.log = io.BytesIO()
        events = []
        def shell(*args):
            if args == ('getprop', 'ro.build.version.sdk'):
                return '35'
            events.append(args)
            return ''
        driver.shell = shell
        def adb(*args, **kwargs):
            values = list(args)
            events.append(('instrument', values[values.index('C4_EXACT_EXPECTED') + 1],
                           values[values.index('C4_NOTIFICATIONS_EXPECTED') + 1]))
            return b'OK (2 tests)'
        driver.adb = adb
        with redirect_stdout(io.StringIO()):
            driver.platform_checks()
        permission = [e for e in events if e[0] in ('pm', 'instrument')]
        self.assertEqual([('pm', 'revoke', gate.APP, 'android.permission.POST_NOTIFICATIONS'),
                          ('instrument', 'granted', 'denied'),
                          ('pm', 'grant', gate.APP, 'android.permission.POST_NOTIFICATIONS'),
                          ('instrument', 'granted', 'granted')], permission)
        self.assertRaises(ValueError, driver.instrument, 'C4PlatformControllerTest', 'granted', notifications='maybe')

    def test_dismissal_instrumentation_relaunches_before_wait(self):
        driver = object.__new__(gate.Gate)
        events = []
        driver.push_json = lambda *a: events.append('fixture')
        driver.android_snapshot = lambda: {'delivery': [{'key': 'n', 'posted': True, 'actionKey': 'original'}]}
        driver.instrument = lambda *a: events.append('instrument')
        driver.shell = lambda *a: events.append(a)
        driver.start_activity = lambda: events.append('launch')
        driver.both = lambda predicate: events.append('wait') or predicate({'delivery': []})
        # Exercise the actual dismissal fixture path, not source text.
        with patch.object(gate, 'wait_for', lambda f, *a: f()):
            driver.dismiss_on_android('n')
        self.assertEqual(['fixture', 'instrument', ('wm', 'dismiss-keyguard'), 'launch', 'wait'], events)

    def test_recovery_checks_never_start_activity(self):
        driver = object.__new__(gate.Gate)
        driver.member = 'self'; driver.items = []; driver.start = 0
        driver.desktop = type('Desktop', (), {'poll': lambda s: None})()
        events = []
        driver.start_activity = lambda: self.fail('activity used as recovery crutch')
        driver.api = lambda method, path, *a: {'id': 'i', 'version': 1}
        driver.android_snapshot = lambda: {'alarms': [{'itemId': 'i', 'code': 1}]}
        driver.alarm_dump = lambda *a: None
        driver.adb = lambda *a: events.append(a)
        driver.booted = lambda: True
        driver.alarm_present = lambda *a: True
        driver.network = lambda value: events.append(('network', value))
        driver.both = lambda f: True
        def shell(*a):
            if a == ('getprop', 'persist.sys.timezone'): return 'Pacific/Auckland'
            if a == ('date', '+%s'): return '120'
            return ''
        driver.shell = shell
        # One network cycle, then exit the thirty-minute loop using a fake clock.
        with patch.object(gate, 'wait_for', lambda f, *a: f()), patch.object(gate.time, 'time', lambda: 0), \
             patch.object(gate.time, 'sleep', lambda *a: None), patch.object(gate.time, 'monotonic', side_effect=[0, 1800, 1800]):
            with redirect_stdout(io.StringIO()) as output:
                driver.soak()
            self.assertIn("PASS:", output.getvalue())
        self.assertIn(('reboot',), events)
        self.assertIn(('network', False), events); self.assertIn(('network', True), events)


if __name__ == '__main__':
    unittest.main()
