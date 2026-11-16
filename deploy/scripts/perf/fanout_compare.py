"""Per-frame legacy/negotiated follow-up workload; no dependencies or hidden retries."""
from concurrent.futures import ThreadPoolExecutor
import datetime as dt
import time
import threading
import urllib.parse
import uuid


FIELDS = ('requests', 'wire_bytes', 'decoded_bytes')


def negotiate(devices):
    available = True
    for device in devices:
        me = device.request('setup.me', 'GET', '/v1/me')
        if not me or not me.get('member_id'): raise ValueError('member discovery failed')
        device.member_id = me['member_id']
        available &= 'item-sync-v1' in me.get('capabilities', [])
    return available


def get(device, path):
    result = device.request('read.targeted', 'GET', path)
    if result is None and device.last_status != 404: raise ValueError('targeted read failed')
    return result


def sync_item(device, item_id):
    path = '/v1/items/'+urllib.parse.quote(item_id, safe='')+'/sync'
    query = [('agenda', device.member_id+':'+':'.join(device.window)), ('include', 'collaboration')]
    seen = set()
    pending = False
    while True:
        encoded = urllib.parse.urlencode(query)
        if encoded in seen: raise ValueError('repeated sync continuation')
        seen.add(encoded)
        page = get(device, path+'?'+encoded)
        if page is None: return False  # Supported endpoint: 404 is authoritative eviction.
        pending |= any(r.get('pending_derivation', False) for r in page.get('items', []))
        coverage = page['coverage']
        query = []
        for agenda in coverage.get('agenda', []):
            if not agenda['complete']:
                if not agenda.get('next_cursor'): raise ValueError('incomplete agenda without cursor')
                query.append(('agenda_cursor', agenda['next_cursor']))
        if coverage.get('comments') and not coverage['comments_complete']:
            cursor = page.get('comments_next_cursor')
            if not cursor: raise ValueError('incomplete comments without cursor')
            query.append(('comments_cursor', cursor))
        if not query: return pending


def targeted_read(device, kind, data):
    """Return pending derivation for a bundle; None means no bundle coverage."""
    if data.get('evicted'): return False
    entity = urllib.parse.quote(str(data.get('entity_id', '')), safe='')
    if kind in ('item.changed', 'item.access_changed', 'item.invalidated', 'derived'):
        return sync_item(device, data['entity_id'])
    if kind in ('occurrence.changed', 'attachment.changed'):
        if not data.get('item_id'): raise ValueError('negotiated child missing parent hint')
        return sync_item(device, data['item_id'])
    if kind == 'comment.changed':
        if not data.get('item_id'): raise ValueError('negotiated comment missing parent hint')
        get(device, '/v1/items/'+urllib.parse.quote(data['item_id'], safe='')+'/comments?ids='+entity)
    elif kind.startswith('notification.'):
        get(device, '/v1/notifications?ids='+entity)
    elif kind in ('list.changed', 'list_entry.changed'):
        if data.get('item_id'): return sync_item(device, data['item_id'])
        listing = data.get('list_id') if kind == 'list_entry.changed' else data.get('entity_id')
        if not listing: raise ValueError('negotiated entry missing list hint')
        get(device, '/v1/lists/'+urllib.parse.quote(listing, safe=''))
    elif kind == 'settings.changed':
        if data.get('entity_type') == 'availability':
            get(device, '/v1/members/'+urllib.parse.quote(device.member_id, safe='')+'/availability')
        else:
            get(device, '/v1/me/settings')
            get(device, '/v1/me/notification-prefs')
    elif kind == 'member.changed':
        get(device, '/v1/members/'+entity)
    elif kind == 'ics_source.changed':
        get(device, '/v1/feeds/'+entity)
    elif kind == 'suggestion.changed':
        device.pages('read.targeted', '/v1/suggestions?limit=200')
    else:
        raise ValueError('unhandled negotiated frame')
    return None


def prepare_edit(writer, devices, index):
    common = set.intersection(*(set(r['id'] for r in d.items) for d in devices))
    editable = [r for r in writer.items if r['id'] in common and 'edit' in r.get('capabilities', [])]
    if not editable: raise ValueError('a writable seeded item visible to every device is required')
    kind = ('title', 'complete', 'recurring', 'comment')[index % 4]
    if kind == 'recurring': editable = [r for r in editable if r.get('rrule')]
    if kind == 'complete': editable = [r for r in editable if 'complete' in r.get('capabilities', [])]
    if not editable: raise ValueError('seeded edit mix unavailable')
    item = editable[0]
    path = '/v1/items/'+item['id']
    current = writer.request('aux.item', 'GET', path)
    if current is None: raise ValueError('version read failed')
    if kind == 'comment':
        return kind, item['id'], ('write.comment', 'POST', path+'/comments', {'body': 'Synthetic fanout '+str(uuid.uuid4())}, None), 'comment.changed', None
    if kind == 'complete':
        # Bounded occurrence collection, not the first historical page outside the retained agenda.
        query = urllib.parse.urlencode({'from': writer.window[0], 'to': writer.window[1], 'undated': 'true', 'limit': 200})
        page = writer.request('aux.occurrences', 'GET', path+'/occurrences?'+query)
        if page is None: raise ValueError('occurrence lookup failed')
        occurrence = next((r for r in page['items'] if r['status'] == 'todo'), None)
        if occurrence is None: raise ValueError('no open seeded occurrence in window')
        return kind, item['id'], ('write.complete', 'POST', '/v1/occurrences/'+occurrence['id']+'/complete', {}, occurrence['version']), 'occurrence.changed', occurrence['id']
    if kind == 'title':
        body = {'title': 'perf-fanout-title-'+str(index % 2)+'-'+str(uuid.uuid4())}
    else:
        # Change RRULE itself, rather than a metadata-only patch. Both runs use the same series.
        body = {'rrule': 'FREQ=DAILY;COUNT='+str(299 if current.get('rrule') == 'FREQ=DAILY;COUNT=300' else 300)}
    return kind, item['id'], ('heavy.recurring-edit' if kind == 'recurring' else 'write.edit', 'PATCH', path, body, current['version']), 'item.changed', item['id']


def follow_edit(device, stream, targeted, start, ack, expected, deadline, quiet, group):
    row = {'stream': stream.index, 'received': False, 'complete': False, 'frames': 0, 'events': []}
    pending = set()
    last_seen = start
    finished = None
    while time.monotonic() < deadline and not device.metrics.stop.is_set():
        with group['lock']:
            with stream.lock:
                batch, stream.events = stream.events, []
            if batch: group['states'][stream.index] = (False, time.monotonic())
        for received, kind, data in batch:
            if received < start: continue
            if kind == 'drain': raise ValueError('server drained during comparison')
            row['frames'] += 1
            last_seen = max(last_seen, received)
            event = {'kind': kind, 'ack_to_event_ms': max(0, received-ack)*1000, 'request_to_event_ms': max(0, received-start)*1000}
            row['events'].append(event)
            native = kind == expected[0] and data.get('entity_id') == expected[1]
            if (native or kind == 'resnapshot') and not row['received']:
                # A terminal reset can supersede the native tickle in an overflowing transaction.
                row['received'] = True
                row.update(delivery_kind=kind, ack_to_event_ms=event['ack_to_event_ms'], request_to_event_ms=event['request_to_event_ms'])
                with device.metrics.lock:
                    device.metrics.sse.add(event['ack_to_event_ms'])
                    device.metrics.sse_upper.add(event['request_to_event_ms'])
            if kind == 'resnapshot':
                pending = device.foreground()
                stream.resume(device.last_snapshot_cursor)
            elif targeted:
                waiting = targeted_read(device, kind, data)
                parent = data.get('item_id') or data.get('entity_id')
                if waiting is True: pending.add(parent)
                elif waiting is False: pending.discard(parent)
            else:
                pending = device.foreground()
            finished = time.monotonic()
        with group['lock']:
            group['states'][stream.index] = (row['received'] and not pending and finished is not None, max(last_seen, finished or start))
            settled = len(group['states']) == len(group['streams']) and all(ok and time.monotonic()-at >= quiet for ok, at in group['states'].values())
            for other in group['streams']:
                with other.lock:
                    if other.events or not other.ready.is_set() or time.monotonic()-getattr(other, 'last_activity', 0) < quiet:
                        settled = False
            if settled: group['done'].set()
        if group['done'].is_set():
            row['complete'] = True
            row['completion_ms'] = max(0, finished-start)*1000
            row['ack_to_completion_ms'] = max(0, finished-ack)*1000
            return row
        device.metrics.stop.wait(0.01)
    row['completion_ms'] = None
    row['pending_derivation'] = bool(pending)
    return row


def delta(after, before):
    return {key: after[key]-before[key] for key in FIELDS}


def compare_fanout(devices, stream_factory, deadline, edits, quiet, output):
    """Populate output incrementally, preserving failed/incomplete edits in raw JSON."""
    available = negotiate(devices)
    # Exactly the same member-specific windows in both back-to-back protocols.
    at = dt.datetime.now(dt.timezone.utc)
    window = ((at-dt.timedelta(days=180)).isoformat(), (at+dt.timedelta(days=548)).isoformat())
    for d in devices: d.window = window
    for name, targeted in [('legacy', False), ('targeted', True)]:
        variant = output[name] = {'status': 'AVAILABLE', 'requested_edits': edits, 'edits': [], 'quiet_seconds': quiet, 'started_at': time.time()}
        if targeted and not available:
            variant.update(status='NOT AVAILABLE', reason='/v1/me lacks item-sync-v1', ended_at=time.time())
            continue
        metrics = devices[0].metrics
        metrics.phase = 'fanout-compare:'+name
        streams = []
        setup_before = [d.traffic_snapshot() for d in devices]
        try:
            for i, d in enumerate(devices):
                d.close()
                stream = stream_factory(d, i, deadline, targeted=targeted)
                stream.managed_reset = True
                streams.append(stream)
                stream.thread.start()
            for stream in streams:
                if not stream.ready.wait(max(0, min(15, deadline-time.monotonic()))): raise ValueError('stream not ready')
            # Do not begin a new edit until a whole quiet period after the previous follow-ups.
            if metrics.stop.wait(quiet): raise ValueError('safety abort')
            variant['stream_setup'] = [delta(d.traffic_snapshot(), b) for d, b in zip(devices, setup_before)]
            with ThreadPoolExecutor(max_workers=len(devices)) as pool:
                for index in range(edits):
                    if time.monotonic() >= deadline: raise TimeoutError('comparison edit limit')
                    before = [d.traffic_snapshot() for d in devices]
                    row = {'edit': index+1, 'devices': [], 'complete': False, 'started_at': time.time()}
                    variant['edits'].append(row)
                    kind, item_id, request, expected_kind, expected_id = prepare_edit(devices[0], devices, index)
                    row['kind'] = kind
                    start = time.monotonic()
                    result = devices[0].request(*request)
                    ack = time.monotonic()
                    row['write_ack_ms'] = (ack-start)*1000
                    if result is None: raise ValueError('comparison mutation failed')
                    expected = (expected_kind, result['id'] if expected_id is None else expected_id)
                    group = {'lock': threading.Lock(), 'done': threading.Event(), 'states': {}, 'streams': streams}
                    jobs = [pool.submit(follow_edit, d, s, targeted, start, ack, expected, deadline, quiet, group) for d, s in zip(devices, streams)]
                    for job in jobs:
                        job.add_done_callback(lambda f: metrics.stop.set() if f.exception() is not None else None)
                    for job, device, baseline in zip(jobs, devices, before):
                        device_row = job.result()
                        device_row.update(delta(device.traffic_snapshot(), baseline))
                        row['devices'].append(device_row)
                    row['ended_at'] = time.time()
                    row['total'] = {k: sum(d[k] for d in row['devices']) for k in FIELDS}
                    row['complete'] = all(d['complete'] for d in row['devices'])
                    row['all_devices_current_ms'] = max(d['completion_ms'] for d in row['devices']) if row['complete'] else None
                    row['ack_to_all_devices_current_ms'] = max(d['ack_to_completion_ms'] for d in row['devices']) if row['complete'] else None
                    if not row['complete']: raise ValueError('missing delivery or unfinished follow-ups')
            variant['complete'] = True
        finally:
            for stream in streams: stream.close()
            for device in devices: device.close()
            variant['ended_at'] = time.time()
            variant['total_including_setup'] = {k: sum(d.traffic_snapshot()[k]-b[k] for d, b in zip(devices, setup_before)) for k in FIELDS}
