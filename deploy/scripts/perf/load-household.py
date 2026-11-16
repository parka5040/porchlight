#!/usr/bin/env python3
"""Bounded pinned-mTLS HTTP/1.1 and SSE load, never raw responses or credentials."""
import argparse
import collections
from concurrent.futures import ThreadPoolExecutor
import datetime as dt
import http.client
import json
import math
from pathlib import Path
import random
import socket
import ssl
import sys
import threading
import time
import urllib.parse
import uuid

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import BodyDecoder, Histogram, SSEParser, private_file, runtime_dir, write_private
from fanout_compare import compare_fanout


def validate_caps(rps, concurrency, duration):
    if not all(math.isfinite(x) for x in (rps, concurrency, duration)) or not 0 < rps <= 50 or not 1 <= concurrency <= 256 or not 0 < duration <= 3600:
        raise ValueError('hard caps: 50 HTTP requests/s, 256 connections, 3600 seconds')


def validate_target(cfg, label, base):
    u = urllib.parse.urlsplit(base)
    if not label or cfg.get('target_label') != label or cfg.get('base_url', '').rstrip('/') != base.rstrip('/'):
        raise ValueError('target label/URL differs from enrolled device target.json')
    if u.scheme != 'https' or not u.hostname or u.username or u.password or u.query or u.fragment or u.path not in ('', '/'):
        raise ValueError('root HTTPS base URL required')
    return u


class Metrics:
    def __init__(self, abort_after):
        self.lock = threading.Lock()
        self.classes = {}
        self.phases = {}
        self.phase = 'setup'
        self.statuses = collections.Counter()
        self.consecutive = 0
        self.abort_after = abort_after
        self.stop = threading.Event()
        self.sse = Histogram()
        self.sse_upper = Histogram()
        self.streams = collections.Counter()
        self.stream_results = []
        self.dropped = 0

    def record(self, cls, status, ms):
        with self.lock:
            h, errors = self.classes.setdefault(cls, [Histogram(), 0])
            h.add(ms)
            if not 200 <= status < 300: self.classes[cls][1] = errors + 1
            phase = self.phases.setdefault(self.phase, {})
            ph, pe = phase.setdefault(cls, [Histogram(), 0])
            ph.add(ms)
            if not 200 <= status < 300: phase[cls][1] = pe + 1
            self.statuses[str(status) if status else 'timeout/transport'] += 1
            self.consecutive = self.consecutive+1 if status == 0 or status >= 500 else 0
            if self.consecutive >= self.abort_after: self.stop.set()

    def output(self):
        with self.lock:
            return {'classes': {k: dict(h.as_dict(), errors=e) for k, (h, e) in sorted(self.classes.items())},
                    'phases': {p: {k: dict(h.as_dict(), errors=e) for k, (h, e) in rows.items()} for p, rows in self.phases.items()},
                    'statuses': dict(self.statuses), 'sse_latency': self.sse.as_dict(),
                    'sse_request_to_event': self.sse_upper.as_dict(), 'streams': dict(self.streams),
                    'stream_results': self.stream_results, 'dropped_operations': self.dropped, 'aborted': self.stop.is_set()}


class Limiter:
    def __init__(self, rps, deadline, metrics):
        self.interval = 1/rps
        self.lock = threading.Lock()
        self.next = time.monotonic()
        self.deadline, self.metrics = deadline, metrics

    def acquire(self):
        with self.lock:
            at = max(time.monotonic(), self.next)
            self.next = at+self.interval
        if at >= self.deadline or self.metrics.stop.wait(max(0, at-time.monotonic())):
            raise TimeoutError('run limit')


class Device:
    def __init__(self, directory, base, label, version, metrics, limiter):
        self.directory = runtime_dir(directory)
        cfg = json.loads(private_file(self.directory / 'target.json').read_text())
        self.endpoint = validate_target(cfg, label, base)
        self.context = ssl.create_default_context(cafile=str(self.directory / 'ca.crt'))
        private_file(self.directory / 'client.key')
        self.context.load_cert_chain(str(self.directory / 'client.crt'), str(self.directory / 'client.key'))
        self.token = private_file(self.directory / 'session.token').read_text().strip()
        self.version, self.metrics, self.limiter = version, metrics, limiter
        self.conn = None
        self.snapshot = None
        self.rng = random.Random(str(directory))
        self.traffic = collections.Counter(requests=0, wire_bytes=0, decoded_bytes=0)
        self.traffic_lock = threading.Lock()
        self.window = agenda_window()
        self.member_id = None
        self.last_status = 0

    def account(self, **values):
        # __new__-constructed offline transports retain the same public request API.
        if not hasattr(self, 'traffic_lock'):
            self.traffic_lock = threading.Lock()
            self.traffic = collections.Counter(requests=0, wire_bytes=0, decoded_bytes=0)
        with self.traffic_lock: self.traffic.update(values)

    def traffic_snapshot(self):
        self.account()
        with self.traffic_lock: return dict(self.traffic)

    def connection(self, timeout=10):
        return http.client.HTTPSConnection(self.endpoint.hostname, self.endpoint.port or 443, context=self.context, timeout=timeout)

    def headers(self, method, match=None):
        h = {'Authorization': 'Bearer '+self.token, 'Porchlight-Client-Version': self.version, 'Accept-Encoding': 'gzip'}
        if method != 'GET': h.update({'Content-Type': 'application/json', 'Idempotency-Key': str(uuid.uuid4())})
        if match is not None: h['If-Match'] = str(match)
        return h

    def request(self, cls, method, path, body=None, match=None, binary=False):
        self.limiter.acquire()
        start = time.monotonic()
        status = 0
        decoder = None
        self.account(requests=1)
        try:
            if self.conn is None: self.conn = self.connection(timeout=max(0.1, min(10, self.limiter.deadline-start)))
            if getattr(self.conn, 'sock', None):
                self.conn.sock.settimeout(max(0.1, min(10, self.limiter.deadline-start)))
            self.conn.request(method, path, json.dumps(body) if body is not None else None, self.headers(method, match))
            reply = self.conn.getresponse()
            status = reply.status
            decoder = BodyDecoder(reply.getheader('Content-Encoding') if hasattr(reply, 'getheader') else None)
            raw = bytearray()
            while chunk := reply.read(65536):
                decoded = decoder.feed(chunk)
                if not binary: raw.extend(decoded)
                if time.monotonic() > self.limiter.deadline: raise TimeoutError('read cap')
            decoder.finish()
            result = {'bytes': decoder.decoded_bytes} if binary else (json.loads(raw) if raw else {})
            if not 200 <= status < 300: return None
            return result
        except (OSError, ValueError, http.client.HTTPException):
            status = status if status >= 500 else 0
            self.close()
            return None
        finally:
            self.last_status = status
            if decoder: self.account(wire_bytes=decoder.wire_bytes, decoded_bytes=decoder.decoded_bytes)
            self.metrics.record(cls, status, (time.monotonic()-start)*1000)

    def close(self):
        if self.conn: self.conn.close()
        self.conn = None

    def pages(self, cls, path):
        seen = set()
        while True:
            page = self.request(cls, 'GET', path)
            if page is None: raise ValueError('collection read failed')
            cursor = page.get('next_cursor')
            if not cursor: return
            if cursor in seen: raise ValueError('repeated cursor')
            seen.add(cursor)
            path = path.split('&cursor=', 1)[0] + '&cursor='+urllib.parse.quote(cursor, safe='')

    def discover(self):
        self.snapshot = self.request('setup.snapshot', 'GET', '/v1/snapshot')
        if not self.snapshot: raise ValueError('cannot discover seeded resources')
        # Retain only routing/permission fields, not N copies of the mature snapshot/history in the load driver.
        self.items = [{k: r.get(k) for k in ('id', 'title', 'capabilities', 'rrule')} for r in self.snapshot['items'] if r['title'].startswith('perf-')]
        self.completable = [r for r in self.items if 'complete' in r.get('capabilities', [])]
        self.attachments = [{'id': r['id']} for r in self.snapshot['attachments'] if r['original_name'].startswith('perf-')]
        self.lists = [{'id': r['id']} for r in self.snapshot['lists'] if r.get('name', '').startswith('perf-') and r['mode'] == 'persistent']
        self.snapshot = None
        if not self.items or not self.completable or not self.attachments or not self.lists:
            raise ValueError('seeded, authorized items/lists/attachments required for each device')

    def foreground(self):
        snapshot = self.request('read.snapshot', 'GET', '/v1/snapshot')
        if snapshot is None: raise ValueError('snapshot read failed')
        self.last_snapshot_cursor = snapshot.get('cursor')
        pending = {r['id'] for r in snapshot['items'] if r.get('pending_derivation')}
        self.pages('read.board', '/v1/board?scope=household&limit=200')
        self.pages('read.agenda', agenda_path(getattr(self, 'window', None), getattr(self, 'member_id', None)))
        return pending

    def operation(self, choice):
        item = self.rng.choice(self.items)
        path = '/v1/items/'+item['id']
        if choice < 30:
            if choice % 2: self.request('read.board', 'GET', '/v1/board?scope=household&limit=200')
            else: self.request('read.agenda', 'GET', agenda_path())
        elif choice < 50:
            kind = choice % 3
            suffix = ['', '/occurrences?undated=true&limit=200', '/comments?limit=50'][kind]
            self.request(['read.item', 'read.occurrences', 'read.comments'][kind], 'GET', path+suffix)
        elif choice < 65:
            self.request('read.lists', 'GET', '/v1/lists?limit=200')
        elif choice < 75:
            self.request('read.notifications', 'GET', '/v1/notifications?limit=50')
        elif choice < 85:
            if choice % 3 == 0:
                self.request('write.comment', 'POST', path+'/comments', {'body': 'Synthetic load '+str(uuid.uuid4())})
            elif choice % 3 == 1:
                owned_path = '/v1/items/'+self.rng.choice(self.completable)['id']
                occs = self.request('aux.occurrences', 'GET', owned_path+'/occurrences?undated=true&limit=200')
                if occs and occs['items']:
                    occ = occs['items'][0]
                    action = 'reopen' if occ['status'] == 'done' else 'complete'
                    self.request('write.'+action, 'POST', '/v1/occurrences/'+occ['id']+'/'+action, {}, occ['version'])
            else:
                listing = self.rng.choice(self.lists)
                current = self.request('aux.list', 'GET', '/v1/lists/'+listing['id'])
                entries = current['entries'] + current.get('got_it', []) if current else []
                if entries:
                    entry = self.rng.choice(entries)
                    action = 'uncheck' if entry.get('checked_at') else 'check'
                    self.request('write.list-'+action, 'POST', '/v1/list-entries/'+entry['id']+'/'+action, {}, entry['version'])
        elif choice < 90: self.request('read.search', 'GET', '/v1/search?q=synthetic&limit=50')
        elif choice < 95: self.request('read.snapshot', 'GET', '/v1/snapshot')
        else: self.request('read.attachment', 'GET', '/v1/attachments/'+self.rng.choice(self.attachments)['id'], binary=True)


def agenda_window():
    at = dt.datetime.now(dt.timezone.utc)
    return ((at-dt.timedelta(days=180)).isoformat(), (at+dt.timedelta(days=548)).isoformat())


def agenda_path(window=None, member_id=None):
    start, end = window or agenda_window()
    query = {'from': start, 'to': end, 'limit': 200}
    if member_id: query['member_id'] = member_id
    return '/v1/agenda?'+urllib.parse.urlencode(query)


class Stream:
    def __init__(self, device, index, deadline, targeted=False):
        self.device, self.index, self.deadline = device, index, deadline
        self.targeted = targeted
        self.after = None
        self.resuming = False
        self.reset = threading.Event()
        self.managed_reset = False
        self.last_activity = time.monotonic()
        self.ready = threading.Event()
        self.quit = threading.Event()
        self.lock = threading.Lock()
        self.events = []
        self.last_id = None
        self.connections = 0
        self.conn = None
        self.thread = threading.Thread(target=self.run, daemon=True)

    def run(self):
        m = self.device.metrics
        while time.monotonic() < self.deadline and not m.stop.is_set() and not self.quit.is_set():
            self.resuming = False
            status = 0
            start = time.monotonic()
            try:
                self.device.limiter.acquire()
                self.conn = self.device.connection(timeout=max(0.1, min(10, self.deadline-start)))
                headers = self.device.headers('GET')
                if self.targeted: headers['Porchlight-Capabilities'] = 'item-sync-v1'
                cursor = self.last_id if self.last_id is not None else self.after
                if cursor is not None: headers['Last-Event-ID'] = str(cursor)
                self.device.account(requests=1)
                self.conn.request('GET', '/v1/stream', headers=headers)
                reply = self.conn.getresponse()
                status = reply.status
                m.record('stream.connect', status, (time.monotonic()-start)*1000)
                if status != 200: raise ValueError('SSE HTTP status')
                # Production heartbeats default to 120 s, not the short normal-request timeout.
                if self.conn.sock:
                    self.conn.sock.settimeout(max(0.1, min(130, self.deadline-time.monotonic())))
                with m.lock:
                    m.streams['connections'] += 1
                    if self.connections: m.streams['reconnects'] += 1
                self.connections += 1
                parser = SSEParser()
                decoder = BodyDecoder(reply.getheader('Content-Encoding'))
                while time.monotonic() < self.deadline and not m.stop.is_set() and not self.quit.is_set():
                    # read1 returns available HTTP entity bytes, not gzip-decoded data.
                    chunk = reply.read1(4096)
                    if not chunk:
                        decoder.finish()
                        break
                    self.last_activity = time.monotonic()
                    decoded = decoder.feed(chunk)
                    self.device.account(wire_bytes=len(chunk), decoded_bytes=len(decoded))
                    for frame in parser.feed(decoded):
                        kind = frame['event']
                        with m.lock: m.streams[kind] += 1
                        if frame.get('id'): self.last_id = frame['id']
                        if kind == 'ready': self.ready.set()
                        elif kind != 'heartbeat':
                            try: data = json.loads(frame['data'])
                            except ValueError: continue
                            if kind == 'resnapshot' and self.managed_reset:
                                self.ready.clear()
                                self.reset.set()
                            with self.lock:
                                self.events.append((time.monotonic(), kind, data))
                                if len(self.events) > 10000:
                                    m.stop.set()
                                    raise ValueError('stream frame backlog cap')
            except (OSError, ValueError, http.client.HTTPException):
                if (not status or status == 200) and not self.quit.is_set() and not self.resuming and time.monotonic() < self.deadline:
                    m.record('stream.error', 0, (time.monotonic()-start)*1000)
            finally:
                self.resuming = False
                if self.conn: self.conn.close()
            while self.reset.is_set() and not self.quit.is_set() and not m.stop.is_set() and time.monotonic() < self.deadline:
                self.quit.wait(0.01)
            self.quit.wait(0.5)

    def resume(self, cursor):
        if cursor is None: raise ValueError('snapshot omitted stream cursor')
        self.last_id = str(cursor)
        self.resuming = True
        self.reset.clear()
        if self.conn and self.conn.sock:
            try: self.conn.sock.shutdown(socket.SHUT_RDWR)
            except OSError: pass

    def close(self):
        self.quit.set()
        if self.conn:
            try:
                if self.conn.sock: self.conn.sock.shutdown(socket.SHUT_RDWR)
            except OSError: pass
            self.conn.close()
        self.thread.join(timeout=11)


def fanout(devices, streams, deadline, heavy):
    writer = devices[0]
    options = [r for r in writer.items if 'edit' in r.get('capabilities', []) and (not heavy or r.get('rrule'))]
    if not options: raise ValueError('no writable seeded series for fanout')
    item = options[0]
    path = '/v1/items/'+item['id']
    while time.monotonic() < deadline and not writer.metrics.stop.is_set():
        current = writer.request('aux.item', 'GET', path)
        if not current: return
        start = time.monotonic()
        # Heavy path changes recurrence, not just metadata, to exercise derivation.
        body = {'rrule': 'FREQ=DAILY;COUNT='+str(299 if current.get('rrule') == 'FREQ=DAILY;COUNT=300' else 300)} if heavy else {'notes': 'Synthetic fanout '+str(uuid.uuid4())}
        result = writer.request('heavy.recurring-edit' if heavy else 'write.edit', 'PATCH', path, body, current['version'])
        commit_ack = time.monotonic()
        if not result: continue
        for stream in streams:
            event = None
            while time.monotonic() < min(deadline, commit_ack+10):
                with stream.lock:
                    event = next((e for e in stream.events if e[0] >= start and e[1] == 'item.changed' and e[2].get('entity_id') == item['id']), None)
                if event: break
                if writer.metrics.stop.wait(0.01): break
            with writer.metrics.lock:
                row = {'stream': stream.index, 'received': event is not None}
                if event:
                    # Exact server commit clock is unavailable. Ack->event is a lower bound, start->event upper.
                    row['ack_to_event_ms'] = max(0, event[0]-commit_ack)*1000
                    row['request_to_event_ms'] = (event[0]-start)*1000
                    writer.metrics.sse.add(row['ack_to_event_ms'])
                    writer.metrics.sse_upper.add(row['request_to_event_ms'])
                writer.metrics.stream_results.append(row)
        if writer.metrics.stop.wait(1): break


def slow_reader(device, deadline, index):
    conn = device.connection(timeout=max(0.1, min(10, deadline-time.monotonic())))
    start = time.monotonic()
    status = 0
    row = {'reader': index, 'resource': 'snapshot' if index % 2 == 0 else 'attachment'}
    try:
        device.limiter.acquire()
        path = '/v1/snapshot' if index % 2 == 0 else '/v1/attachments/'+device.attachments[0]['id']
        conn.request('GET', path, headers=device.headers('GET'))
        reply = conn.getresponse()
        status = reply.status
        row.update(status=status, content_length=reply.getheader('Content-Length'), outcome='held-unread')
        # Do not call reply.read. TLS buffering can hide peer close; do not claim eviction unless observable.
        device.metrics.stop.wait(max(0, deadline-time.monotonic()))
        row['held_seconds'] = time.monotonic()-start
        row['peer_close'] = 'not-observable-without-consuming-TLS-body'
    except (OSError, ValueError, http.client.HTTPException): row['outcome'] = 'closed-or-timeout-before-headers'
    finally:
        conn.close()
        device.metrics.record('slow.headers', status, (time.monotonic()-start)*1000)
        with device.metrics.lock: device.metrics.stream_results.append(row)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('scenario', choices=['ladder', 'foreground', 'edit-fanout', 'fanout-compare', 'slow-readers', 'sse-hold'])
    for key in ['devices', 'base-url', 'target-label', 'out']: p.add_argument('--'+key, required=True)
    version_file = Path(__file__).resolve().parents[3] / 'VERSION'
    p.add_argument('--client-version', default=version_file.read_text().strip() if version_file.is_file() else None)
    p.add_argument('--max-rps', type=float, default=50)
    p.add_argument('--max-concurrency', type=int, default=32)
    p.add_argument('--duration', type=float, help='hard wall-clock limit (compare: 600 s; others: 60 s)')
    p.add_argument('--steps', default='1,10,25,50')
    p.add_argument('--step-duration', type=float, default=10)
    p.add_argument('--abort-after', type=int, default=5)
    p.add_argument('--streams', type=int, help='distinct compare devices (default 12); other stream scenarios default 6')
    p.add_argument('--edits', type=int, default=20)
    p.add_argument('--settle-seconds', type=float, default=1, help='quiet period after all frame-triggered reads; excluded from completion latency')
    p.add_argument('--readers', type=int, default=4)
    p.add_argument('--heavy-recurring-edit', action='store_true')
    a = p.parse_args()
    if a.duration is None: a.duration = 600 if a.scenario == 'fanout-compare' else 60
    if a.streams is None: a.streams = 12 if a.scenario == 'fanout-compare' else 6
    validate_caps(a.max_rps, a.max_concurrency, a.duration)
    if not 1 <= a.edits <= 200: raise ValueError('edit cap is 200')
    if not math.isfinite(a.settle_seconds) or not 0.1 <= a.settle_seconds <= 30: raise ValueError('quiet period must be 0.1–30 seconds')
    if not a.client_version: raise ValueError('--client-version required outside a checkout')
    if not 1 <= a.abort_after <= 100: raise ValueError('abort-after cap is 100')
    out = runtime_dir(a.out)
    if out.name == 'var': raise ValueError('output file required')
    cfg = json.loads(private_file(a.devices).read_text())
    directories = cfg['devices']
    if not 1 <= len(directories) <= a.max_concurrency: raise ValueError('devices exceed concurrency cap')
    if a.scenario in ('sse-hold', 'edit-fanout', 'fanout-compare') and not 1 <= a.streams <= a.max_concurrency: raise ValueError('stream cap exceeded')
    if a.scenario == 'fanout-compare':
        if a.streams > len(directories): raise ValueError('compare requires distinct enrolled devices; provision more devices')
        if 2*a.streams > a.max_concurrency: raise ValueError('compare requires two connections per device (stream + reads)')
        directories = directories[:a.streams]
    if a.scenario == 'slow-readers' and not 1 <= a.readers <= a.max_concurrency: raise ValueError('reader cap exceeded')
    if a.scenario == 'edit-fanout' and a.streams+1 > a.max_concurrency: raise ValueError('fanout requires one additional writer connection')
    steps = [float(v) for v in a.steps.split(',')]
    if a.scenario == 'ladder':
        if any(not math.isfinite(v) or not 0 < v <= a.max_rps for v in steps): raise ValueError('ladder step exceeds max-rps')
        if not math.isfinite(a.step_duration) or a.step_duration <= 0 or len(steps)*a.step_duration > a.duration: raise ValueError('ladder exceeds duration cap')
    # Validate every target before opening ANY network connection.
    for directory in directories:
        validate_target(json.loads(private_file(runtime_dir(directory) / 'target.json').read_text()), a.target_label, a.base_url)
    metrics = Metrics(a.abort_after)
    started = time.time()
    deadline = time.monotonic()+a.duration
    limiter = Limiter(a.max_rps, deadline, metrics)
    devices = [Device(d, a.base_url, a.target_label, a.client_version, metrics, limiter) for d in directories]
    streams = []
    failed = False
    comparison = {}
    try:
        if a.scenario not in ('foreground', 'sse-hold'):
            for device in devices: device.discover()
            # Discovery keep-alives must not add to the persistent-stream/slow-reader cap.
            if a.scenario in ('edit-fanout', 'slow-readers'):
                for device in devices: device.close()
        metrics.phase = a.scenario
        if a.scenario == 'fanout-compare':
            compare_fanout(devices, Stream, deadline, a.edits, a.settle_seconds, comparison)
        elif a.scenario == 'foreground':
            barrier = threading.Barrier(len(devices))
            def refresh(device):
                barrier.wait(timeout=10)
                device.foreground()
            with ThreadPoolExecutor(max_workers=len(devices)) as pool: list(pool.map(refresh, devices))
        elif a.scenario == 'ladder':
            # One outstanding operation per device; no unbounded executor queue or catch-up bursts.
            with ThreadPoolExecutor(max_workers=len(devices)) as pool:
                free = list(devices)
                pending = {}
                for rate in steps:
                    metrics.phase = f'ladder:{rate:g}rps'
                    phase_end = min(deadline, time.monotonic()+a.step_duration)
                    tick = time.monotonic()
                    while time.monotonic() < phase_end and not metrics.stop.is_set():
                        for f in list(pending):
                            if f.done():
                                f.result()
                                free.append(pending.pop(f))
                        if free:
                            d = free.pop()
                            pending[pool.submit(d.operation, d.rng.randrange(100))] = d
                        else: metrics.dropped += 1
                        tick += 1/rate
                        metrics.stop.wait(max(0, tick-time.monotonic()))
                    for f in pending: f.result()
                    free += list(pending.values())
                    pending = {}
        elif a.scenario in ('sse-hold', 'edit-fanout'):
            for i in range(a.streams):
                s = Stream(devices[i % len(devices)], i, deadline)
                streams.append(s)
                s.thread.start()
            for s in streams:
                if not s.ready.wait(max(0, min(15, deadline-time.monotonic()))): raise ValueError('stream not ready')
            if a.scenario == 'edit-fanout': fanout(devices, streams, deadline, a.heavy_recurring_edit)
            else: metrics.stop.wait(max(0, deadline-time.monotonic()))
        else:
            with ThreadPoolExecutor(max_workers=a.readers) as pool:
                futures = [pool.submit(slow_reader, devices[i % len(devices)], deadline, i) for i in range(a.readers)]
                for f in futures: f.result()
    except TimeoutError:
        failed = a.scenario == 'fanout-compare' or time.monotonic() < deadline
    except Exception:
        failed = True
    finally:
        for s in streams: s.close()
        for d in devices: d.close()
        result = dict(metrics.output(), schema=1, scenario=a.scenario, target_label=a.target_label, started_at=started,
                      ended_at=time.time(), failed=failed, caps={'rps': a.max_rps, 'concurrency': a.max_concurrency, 'duration': a.duration},
                      mix='30/20/15/10/10/5/5/5 logical operations; version reads counted separately as aux',
                      sse_clock='ack-to-event lower bound; request-to-event conservative upper bound; no server commit timestamp')
        if a.scenario == 'fanout-compare':
            result['fanout_compare'] = comparison
            result['byte_accounting'] = 'response HTTP entity bytes (wire=gzip if negotiated), excluding headers/TLS; per-edit includes writer/auxiliary reads and delivered SSE chunks; setup separate'
        write_private(out, json.dumps(result, indent=2))
        print('endpoint class                    count errors    p50    p95    p99    max ms')
        for cls, row in result['classes'].items():
            print(f'{cls:32} {row["count"]:6} {row["errors"]:6} '+ ' '.join(f'{row[k]:6.1f}' for k in ['p50_ms', 'p95_ms', 'p99_ms', 'max_ms']))
        if failed or result['aborted']: raise ValueError('run failed or safety abort')

if __name__ == '__main__':
    try: main()
    except Exception:
        print('FAIL: load refused, failed, or aborted (raw replies suppressed)', file=sys.stderr)
        sys.exit(1)
