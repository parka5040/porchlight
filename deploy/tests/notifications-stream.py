#!/usr/bin/env python3
"""Offline SSE parser regressions for the controller-only notification gate."""
import io
from pathlib import Path
import runpy
import unittest

module = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/m5-notifications-demo.py'))


class Frames(unittest.TestCase):
    def test_skips_eager_heartbeats_and_other_frames(self):
        reply = io.BytesIO(b': heartbeat\n\nevent: ready\ndata: {"cursor":0}\n\nevent: item.changed\ndata: {"entity_id":"item"}\n\nevent: notification.dismissed\ndata: {"entity_id":"inbox"}\n\n')
        self.assertEqual(module['read_sse_until'](reply, 'notification.dismissed', 'inbox')['entity_id'], 'inbox')

    def test_other_member_cannot_receive_dismissal_before_barrier(self):
        reply = io.BytesIO(b'event: notification.dismissed\ndata: {"entity_id":"inbox"}\n\nevent: item.changed\ndata: {"entity_id":"barrier"}\n\n')
        with self.assertRaisesRegex(ValueError, 'recipient isolation'):
            module['read_sse_until'](reply, 'item.changed', 'barrier', forbid_notifications={'inbox'})

    def test_eof_is_not_success(self):
        with self.assertRaisesRegex(ValueError, 'stream ended'):
            module['read_sse_until'](io.BytesIO(b': heartbeat\n\n'), 'notification.dismissed', 'inbox')


if __name__ == '__main__':
    unittest.main()
