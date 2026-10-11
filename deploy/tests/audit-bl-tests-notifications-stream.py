#!/usr/bin/env python3
"""The privacy barrier ignores unrelated notifications but rejects fixture leaks."""
import io
from pathlib import Path
import runpy
import unittest

module = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/m5-notifications-demo.py'))


class NotificationCausality(unittest.TestCase):
    def test_unrelated_notification_does_not_fail_privacy_barrier(self):
        for kind in ['notification.created', 'notification.updated', 'notification.dismissed']:
            with self.subTest(kind=kind):
                reply = io.BytesIO((f'event: {kind}\ndata: {{"entity_id":"other-inbox"}}\n\n'
                                   'event: item.changed\ndata: {"entity_id":"barrier"}\n\n').encode())
                result = module['read_sse_until'](reply, 'item.changed', 'barrier',
                                                  forbid_notifications={'fixture-inbox'})
                self.assertEqual(result['entity_id'], 'barrier')

    def test_each_fixture_notification_kind_is_rejected(self):
        for kind in ['notification.created', 'notification.updated', 'notification.dismissed']:
            with self.subTest(kind=kind):
                reply = io.BytesIO((f'event: {kind}\ndata: {{"entity_id":"fixture-inbox"}}\n\n'
                                   'event: item.changed\ndata: {"entity_id":"barrier"}\n\n').encode())
                with self.assertRaisesRegex(ValueError, 'recipient isolation'):
                    module['read_sse_until'](reply, 'item.changed', 'barrier',
                                             forbid_notifications={'fixture-inbox'})


if __name__ == '__main__':
    unittest.main()
