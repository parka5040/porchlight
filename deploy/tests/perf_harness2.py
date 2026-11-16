"""Recorded-byte and offline fanout comparison contracts."""
import importlib.util
import gzip
import json
from pathlib import Path
import sys
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'deploy/scripts/perf'))


def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'deploy/scripts/perf' / (name+'.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


load = module('load-household')
report = module('report')


class FanoutCompare(unittest.TestCase):
    def test_recorded_gzip_wire_and_decoded_bytes(self):
        # gzip -n equivalent, recorded JSON response {"version":2}.
        wire = bytes.fromhex('1f8b08000000000002ffab562a4b2d2acecccf53b232aa0500f0c91d5a0d000000')
        class Reply:
            status = 200
            def getheader(self, key, default=None): return 'gzip' if key == 'Content-Encoding' else default
            def read(self, size):
                chunk, self.body = self.body[:3], self.body[3:]
                return chunk
            body = wire
        class Conn:
            def request(self, *args): self.headers = args[3]
            def getresponse(self): return Reply()
            def close(self): pass
        class Limiter:
            deadline = time.monotonic()+10
            def acquire(self): pass
        d = load.Device.__new__(load.Device)
        d.conn, d.token, d.version = Conn(), 'fixture', '0.1.8'
        d.metrics, d.limiter = load.Metrics(3), Limiter()
        self.assertEqual(d.request('read.item', 'GET', '/v1/items/fixture'), {'version': 2})
        self.assertEqual(d.conn.headers['Accept-Encoding'], 'gzip')
        self.assertEqual(d.traffic_snapshot(), {'requests': 1, 'wire_bytes': len(wire), 'decoded_bytes': 13})

    def test_stream_opt_in_gzip_and_reconnect_cursor(self):
        fixture = b'event: ready\ndata: {"cursor":7}\n\nid: 9\nevent: item.invalidated\ndata: {"entity_id":"item-fixture"}\n\n'
        packed = gzip.compress(fixture, mtime=0)
        for targeted in (False, True):
            class Reply:
                status = 200
                body = packed
                def getheader(self, name): return 'gzip'
                def read1(self, size):
                    chunk, self.body = self.body[:3], self.body[3:]
                    if not chunk: stream.quit.set()
                    return chunk
            class Conn:
                sock = None
                def request(self, method, path, headers): self.headers = headers
                def getresponse(self): return Reply()
                def close(self): pass
            class Limiter:
                deadline = time.monotonic()+1
                def acquire(self): pass
            d = load.Device.__new__(load.Device)
            d.token, d.version, d.metrics, d.limiter = 'fixture', '0.1.8', load.Metrics(3), Limiter()
            conn = Conn()
            d.connection = lambda timeout: conn
            stream = load.Stream(d, 0, time.monotonic()+1, targeted=targeted)
            stream.after = 7
            stream.run()
            self.assertTrue(stream.ready.is_set())
            self.assertEqual(stream.last_id, '9')
            self.assertEqual(conn.headers['Last-Event-ID'], '7')
            self.assertEqual(conn.headers['Accept-Encoding'], 'gzip')
            self.assertEqual(conn.headers.get('Porchlight-Capabilities'), 'item-sync-v1' if targeted else None)
            self.assertEqual(d.traffic_snapshot(), {'requests': 1, 'wire_bytes': len(packed), 'decoded_bytes': len(fixture)})
            self.assertEqual(stream.events[0][1], 'item.invalidated')

    def test_sync_continuations_and_own_window(self):
        compare = module('fanout_compare')
        pages = [
            b'{"items":[{"pending_derivation":false}],"coverage":{"agenda":[{"complete":false,"next_cursor":"a+1"}],"comments":true,"comments_complete":false},"comments_next_cursor":"c+1"}',
            b'{"coverage":{"agenda":[{"complete":false,"next_cursor":"a2"}],"comments":true,"comments_complete":true}}',
            b'{"coverage":{"agenda":[{"complete":true,"next_cursor":null}]}}',
        ]
        class Device:
            member_id, window = 'member-fixture', ('2026-05-01T00:00:00+00:00', '2028-04-28T00:00:00+00:00')
            last_status = 200
            def request(self, cls, method, path):
                calls.append(path)
                return json.loads(pages.pop(0))
        calls = []
        self.assertFalse(compare.targeted_read(Device(), 'item.invalidated', {'entity_id': 'item-fixture'}))
        self.assertIn('agenda=member-fixture%3A', calls[0])
        self.assertIn('include=collaboration', calls[0])
        self.assertEqual(calls[1], '/v1/items/item-fixture/sync?agenda_cursor=a%2B1&comments_cursor=c%2B1')
        self.assertEqual(calls[2], '/v1/items/item-fixture/sync?agenda_cursor=a2')

    def test_exact_ids_and_no_binary_attachment_download(self):
        compare = module('fanout_compare')
        class Device:
            member_id, window, last_status = 'member-fixture', ('from', 'to'), 200
            def request(self, cls, method, path):
                calls.append(path)
                return {'coverage': {}, 'items': []}
        calls = []
        d = Device()
        compare.targeted_read(d, 'comment.changed', {'entity_id': 'comment-fixture', 'item_id': 'item-fixture'})
        compare.targeted_read(d, 'notification.invalidated', {'entity_id': 'notification-fixture'})
        compare.targeted_read(d, 'attachment.changed', {'entity_id': 'attachment-fixture', 'item_id': 'item-fixture'})
        self.assertEqual(calls[:2], ['/v1/items/item-fixture/comments?ids=comment-fixture', '/v1/notifications?ids=notification-fixture'])
        self.assertTrue(calls[2].startswith('/v1/items/item-fixture/sync?'))
        with self.assertRaises(ValueError): compare.targeted_read(d, 'occurrence.changed', {'entity_id': 'no-parent'})

    def test_capability_absent_is_not_available(self):
        compare = module('fanout_compare')
        devices = []
        for raw in (b'{"member_id":"first"}', b'{"member_id":"second","capabilities":[]}'):
            class Device:
                def request(self, *args): return json.loads(self.raw)
            d = Device()
            d.raw = raw
            devices.append(d)
        self.assertFalse(compare.negotiate(devices))
        self.assertEqual([d.member_id for d in devices], ['first', 'second'])

    def test_back_to_back_protocols_and_every_delivered_frame(self):
        compare = module('fanout_compare')
        metrics = load.Metrics(3)
        devices, opened = [], []
        class Device:
            window = ('from', 'to')
            last_status = 200
            items = [{'id': 'item-fixture', 'capabilities': ['edit', 'complete'], 'rrule': 'FREQ=DAILY;COUNT=300'}]
            def __init__(self): self.traffic = dict(requests=0, wire_bytes=0, decoded_bytes=0); self.metrics = metrics
            def traffic_snapshot(self): return self.traffic.copy()
            def close(self): pass
            def pages(self, *args): raise AssertionError('unexpected collection')
            def request(self, cls, method, path, body=None, match=None):
                self.traffic['requests'] += 1
                self.traffic['wire_bytes'] += 10
                self.traffic['decoded_bytes'] += 100
                if path == '/v1/me': return {'member_id': 'member-fixture', 'capabilities': ['item-sync-v1']}
                if method == 'GET':
                    if '/sync?' in path: return {'items': [{'pending_derivation': False}], 'coverage': {}}
                    if '/comments?ids=' in path: return {'items': [], 'next_cursor': None}
                    if '/occurrences?' in path: return {'items': [{'id': 'occ-fixture', 'status': 'todo', 'version': 1}]}
                    return {'id': 'item-fixture', 'version': 1, 'rrule': 'FREQ=DAILY;COUNT=300'}
                kind, entity = ('comment.changed', 'comment-fixture') if path.endswith('/comments') else (('occurrence.changed', 'occ-fixture') if path.endswith('/complete') else ('item.changed', 'item-fixture'))
                for device in devices:
                    with device.stream.lock:
                        device.stream.events.append((time.monotonic(), kind, {'entity_id': entity, 'item_id': 'item-fixture'}))
                        # Extra projection/derived frames must each cause another read, not be dropped.
                        device.stream.events.append((time.monotonic(), 'derived', {'entity_id': 'item-fixture'}))
                        if device.stream.targeted:
                            device.stream.events.append((time.monotonic(), 'item.invalidated', {'entity_id': 'item-fixture'}))
                return {'id': entity}
            def foreground(self):
                self.traffic['requests'] += 3
                self.traffic['wire_bytes'] += 1000
                self.traffic['decoded_bytes'] += 10000
                return set()
        class Stream:
            def __init__(self, d, index, deadline, targeted=False):
                self.index, self.targeted, self.events = index, targeted, []
                self.ready, self.lock = threading.Event(), threading.Lock()
                self.thread = threading.Thread(target=self.ready.set)
                d.stream = self
                opened.append(targeted)
            def close(self): self.thread.join()
        devices.extend([Device(), Device()])
        result = {}
        compare.compare_fanout(devices, Stream, time.monotonic()+5, 4, 0.01, result)
        self.assertEqual(opened, [False, False, True, True])
        for name, frames in [('legacy', 2), ('targeted', 3)]:
            self.assertTrue(result[name]['complete'])
            self.assertEqual([e['kind'] for e in result[name]['edits']], ['title', 'complete', 'recurring', 'comment'])
            for edit in result[name]['edits']:
                self.assertTrue(edit['complete'])
                self.assertEqual(edit['devices'][1]['frames'], frames)
                self.assertEqual(edit['devices'][1]['requests'], 6 if name == 'legacy' else 3)
                self.assertEqual(edit['total']['requests'], sum(d['requests'] for d in edit['devices']))
        # Real runner must skip the targeted stream and retain the legacy result on old servers.
        original = devices[0].request
        def old_server(cls, method, path, *args):
            return {'member_id': 'member-fixture'} if path == '/v1/me' else original(cls, method, path, *args)
        devices[0].request = old_server
        opened.clear()
        result = {}
        compare.compare_fanout(devices, Stream, time.monotonic()+5, 1, 0.01, result)
        self.assertEqual(opened, [False, False])
        self.assertEqual(result['targeted']['status'], 'NOT AVAILABLE')

    def test_quiet_device_keeps_reading_while_peer_is_busy(self):
        compare = module('fanout_compare')
        metrics = load.Metrics(3)
        class Device:
            def __init__(self, slow): self.metrics, self.slow, self.reads = metrics, slow, 0
            def foreground(self):
                self.reads += 1
                if self.slow: time.sleep(0.08)
                return set()
        class Stream:
            def __init__(self, index):
                self.index, self.lock = index, threading.Lock()
                self.ready = threading.Event()
                self.ready.set()
                self.events = [(start, 'item.changed', {'entity_id': 'fixture'})]
        start = time.monotonic()
        devices, streams = [Device(False), Device(True)], [Stream(0), Stream(1)]
        group = {'lock': threading.Lock(), 'done': threading.Event(), 'states': {}, 'streams': streams}
        def later_frame():
            with streams[0].lock: streams[0].events.append((time.monotonic(), 'derived', {'entity_id': 'fixture'}))
        timer = threading.Timer(0.04, later_frame)
        timer.start()
        with compare.ThreadPoolExecutor(max_workers=2) as pool:
            jobs = [pool.submit(compare.follow_edit, d, s, False, start, start, ('item.changed', 'fixture'), start+1, 0.01, group) for d, s in zip(devices, streams)]
            rows = [j.result() for j in jobs]
        timer.join()
        self.assertEqual(devices[0].reads, 2, 'a locally quiet device must not exit before its busy peer')
        self.assertTrue(all(r['complete'] for r in rows))

    def test_terminal_reset_refreshes_and_waits_for_fresh_ready(self):
        compare = module('fanout_compare')
        class Device:
            metrics = load.Metrics(3)
            last_snapshot_cursor = 12
            def foreground(self):
                self.refreshed = True
                return set()
        class Stream:
            index, lock = 0, threading.Lock()
            ready = threading.Event()
            def resume(self, cursor):
                self.cursor = cursor
                self.timer = threading.Timer(0.03, self.ready.set)
                self.timer.start()
        start = time.monotonic()
        device, stream = Device(), Stream()
        stream.events = [(start, 'resnapshot', {'reason': 'projection_changed'})]
        group = {'lock': threading.Lock(), 'done': threading.Event(), 'states': {}, 'streams': [stream]}
        row = compare.follow_edit(device, stream, True, start, start, ('item.changed', 'fixture'), start+1, 0.01, group)
        stream.timer.join()
        self.assertTrue(device.refreshed)
        self.assertEqual(stream.cursor, 12)
        self.assertEqual(row['delivery_kind'], 'resnapshot')
        self.assertTrue(row['complete'])
        self.assertTrue(stream.ready.is_set())

    def test_missing_delivery_is_not_a_completion_sample(self):
        compare = module('fanout_compare')
        class Device:
            metrics = load.Metrics(3)
        class Stream:
            index, events, lock = 0, [], threading.Lock()
            ready = threading.Event()
        stream = Stream()
        stream.ready.set()
        group = {'lock': threading.Lock(), 'done': threading.Event(), 'states': {}, 'streams': [stream]}
        start = time.monotonic()
        row = compare.follow_edit(Device(), stream, True, start, start, ('item.changed', 'fixture'), start+0.02, 0.01, group)
        self.assertFalse(row['received'])
        self.assertFalse(row['complete'])
        self.assertIsNone(row['completion_ms'])

    def test_bounded_gzip_and_truncation(self):
        common = module('common')
        wire = bytes.fromhex('1f8b08000000000002ffab562a4b2d2acecccf53b232aa0500f0c91d5a0d000000')
        decoder = common.BodyDecoder('gzip')
        decoder.feed(wire[:-2])
        with self.assertRaises(ValueError): decoder.finish()
        with self.assertRaises(ValueError): common.BodyDecoder('gzip', cap=12).feed(wire)
        with self.assertRaises(ValueError): common.BodyDecoder('br')

    def test_report_comparison_and_targeted_budgets(self):
        budgets = json.loads((ROOT / 'deploy/scripts/perf/budgets.json').read_text())
        def variant(wire, ms):
            return {'status': 'AVAILABLE', 'edits': [{'devices': [
                {'requests': 2, 'wire_bytes': wire, 'decoded_bytes': 40000, 'completion_ms': ms, 'received': True}],
                'total': {'requests': 2, 'wire_bytes': wire, 'decoded_bytes': 40000},
                'all_devices_current_ms': ms}]}
        sample = {'scenario': 'fanout-compare', 'classes': {}, 'statuses': {}, 'fanout_compare': {
            'legacy': variant(20000, 100), 'targeted': variant(100001, 2001)}}
        text = report.render(sample, [], budgets)
        self.assertIn('## Fanout comparison', text)
        self.assertIn('wire MB/edit', text)
        self.assertIn('| targeted wire bytes/device/edit p95 | FAIL', text)
        self.assertIn('| targeted all devices current p95 ms | FAIL', text)
        sample['fanout_compare']['targeted'] = {'status': 'NOT AVAILABLE', 'reason': 'capability absent', 'edits': []}
        self.assertIn('NOT AVAILABLE', report.render(sample, [], budgets))
        self.assertIn('**Overall: INCOMPLETE**', report.render(sample, [], budgets))
        sample.update(target_label='fixture', started_at=1, ended_at=2)
        merged = report.merge_loads([sample, sample])
        self.assertEqual(len(merged['fanout_compare']['legacy']['edits']), 2)


if __name__ == '__main__': unittest.main()
