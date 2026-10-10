#!/usr/bin/env python3
"""Provisioned-stack notification gate. Config: base_url, device_dir, household_id, recipient_member_id.
Device dir holds ca.crt/client.crt/client.key/session.token from porch dev-init. Use an empty test household.
Source scripts/env.sh first. Connects to local PG as the agent; never outputs SQL, replies or secrets.
Requires second_device_dir (same member), other_member_device_dir (different member).
After this gate, chains S6b stream/replay/eviction/resnapshot checks on the same database.
"""
import datetime as dt
import http.client
import json
import os
from pathlib import Path
import ssl
import subprocess
import sys
import time
import urllib.parse
import uuid
from zoneinfo import ZoneInfo

ROOT = Path(__file__).resolve().parents[2]
VAR = (ROOT / 'var').resolve()

def private(path):
    resolved = Path(path).resolve()
    if not resolved.is_relative_to(VAR):
        raise ValueError('runtime path outside var')
    return resolved

def read_sse_until(reply, event, entity_id=None, forbid_notifications=False):
    """Read past eager heartbeats/unrelated frames, with one bounded deadline."""
    deadline = time.monotonic() + 15
    fields = {}
    while time.monotonic() < deadline:
        raw = reply.readline(65537)
        if not raw:
            raise ValueError('stream ended before expected event')
        if len(raw) > 65536:
            raise ValueError('oversized stream frame')
        line = raw.decode('utf-8').rstrip('\r\n')
        if line.startswith(':'):
            continue
        if not line:
            kind = fields.get('event', '')
            if forbid_notifications and kind.startswith('notification.'):
                raise ValueError('recipient isolation failed')
            if kind == event:
                data = json.loads(fields.get('data', '{}'))
                if entity_id is None or data.get('entity_id') == entity_id:
                    return data
            fields = {}
        elif ':' in line:
            key, value = line.split(':', 1)
            fields[key] = value.lstrip(' ')
    raise ValueError('stream deadline exceeded')

def run(config):
    cfg = json.loads(private(config).read_text())
    device = private(cfg['device_dir'])
    endpoint = urllib.parse.urlsplit(cfg['base_url'])
    if endpoint.scheme != 'https' or endpoint.username or endpoint.password:
        raise ValueError('HTTPS endpoint required')
    household = str(uuid.UUID(cfg['household_id']))
    recipient = str(uuid.UUID(cfg['recipient_member_id']))
    def credentials(directory):
        directory = private(directory)
        ctx = ssl.create_default_context(cafile=str(directory / 'ca.crt'))
        ctx.load_cert_chain(str(directory / 'client.crt'), str(directory / 'client.key'))
        return ctx, (directory / 'session.token').read_text().strip()
    context, session = credentials(device)
    second = credentials(cfg['second_device_dir'])
    other = credentials(cfg['other_member_device_dir'])
    streams = []
    version = (ROOT / 'VERSION').read_text().strip()
    def api(method, path, body=None, match=None, caller=None):
        ctx, token = caller or (context, session)
        conn = http.client.HTTPSConnection(endpoint.hostname, endpoint.port or 443, context=ctx, timeout=15)
        headers = {'Authorization': 'Bearer ' + token, 'Porchlight-Client-Version': version}
        if method != 'GET':
            headers.update({'Content-Type': 'application/json', 'Idempotency-Key': str(uuid.uuid4())})
        if match is not None:
            headers['If-Match'] = str(match)
        try:
            conn.request(method, endpoint.path.rstrip('/') + path, json.dumps(body) if body is not None else None, headers)
            reply = conn.getresponse()
            raw = reply.read(4 * 1024 * 1024)
            if reply.status != 200:
                raise ValueError('unexpected HTTP status')
            return json.loads(raw)
        finally:
            conn.close()
    def stream(caller):
        ctx, token = caller
        conn = http.client.HTTPSConnection(endpoint.hostname, endpoint.port or 443, context=ctx, timeout=15)
        try:
            conn.request('GET', endpoint.path.rstrip('/') + '/v1/stream', headers={
                'Authorization': 'Bearer ' + token, 'Porchlight-Client-Version': version})
            reply = conn.getresponse()
            if reply.status != 200:
                raise ValueError('stream authentication failed')
            read_sse_until(reply, 'ready')
            streams.append((conn, reply))
            return reply
        except Exception:
            conn.close()
            raise
    # Parse credentials only into a child environment (never arguments/diagnostics).
    db = urllib.parse.urlsplit(os.environ['PORCHLIGHT_SUPERUSER_URL'])
    query = urllib.parse.parse_qs(db.query)
    pg = dict(os.environ)
    pg.update(PGHOST=query.get('host', [db.hostname or ''])[0], PGPORT=str(db.port or 5432),
              PGDATABASE=urllib.parse.unquote(db.path.lstrip('/')), PGUSER='porchlight_agent',
              PGPASSWORD=os.environ['PORCHLIGHT_AGENT_PASSWORD'])
    def sql(statement):
        result = subprocess.run(['psql', '-X', '-q', '-v', 'ON_ERROR_STOP=1'], input=statement,
                                text=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=pg)
        if result.returncode:
            raise ValueError('SQL fixture failed')
    def wait_for(predicate, timeout=15):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            found = predicate(api('GET', '/v1/notifications?limit=200')['items'])
            if found:
                return found
            time.sleep(0.25)
        raise ValueError('notification wait timed out')
    initial = api('GET', '/v1/me/notification-prefs')
    if initial['member_id'] != recipient:
        raise ValueError('config recipient does not match session')
    if api('GET', '/v1/me/notification-prefs', caller=second)['member_id'] != recipient:
        raise ValueError('second device is not the recipient')
    if api('GET', '/v1/me/notification-prefs', caller=other)['member_id'] == recipient:
        raise ValueError('other member device is not independent')
    profiles = api('GET', '/v1/members')['items']
    member = next(m for m in profiles if m['id'] == recipient)
    zone = ZoneInfo(member['tzid'])
    items = [str(uuid.uuid4()) for _ in range(3)]
    prefs = dict(initial['prefs'])
    prefs.update(assignments=True, activity=True, digest=True, reminders=True, quiet_start=None, quiet_end=None)
    current = api('PUT', '/v1/me/notification-prefs', prefs, initial['version'])
    try:
        # Agent assignment trigger is the real producer; two inserts share a 30-s window.
        values = ','.join(f"('{i}','{household}','{recipient}','M5 demo fixture','todo','private')" for i in items)
        sql(f"BEGIN; SET LOCAL application_name='agent:m5-demo'; INSERT INTO porchlight.items(id,household_id,owner_id,title,kind,visibility) VALUES {values}; INSERT INTO porchlight.item_assignees(household_id,item_id,member_id) VALUES ('{household}','{items[0]}','{recipient}'),('{household}','{items[1]}','{recipient}'); COMMIT;")
        n = wait_for(lambda rows: next((n for n in rows if n['count'] == 2 and n.get('actor_label') == 'agent:m5-demo' and n['summary']['text'].startswith('agent:m5-demo')), None))
        print('PASS: agent assignment routing and 30-second coalescing', flush=True)
        first_stream = stream((context, session))
        second_stream = stream(second)
        other_stream = stream(other)
        read = api('POST', f"/v1/notifications/{n['id']}/read", {})
        if not read['read_at']:
            raise ValueError('read action failed')
        read_sse_until(first_stream, 'notification.dismissed', n['id'])
        read_sse_until(second_stream, 'notification.dismissed', n['id'])
        if any(row['id'] == n['id'] for row in api('GET', '/v1/notifications?unread=true', caller=second)['items']):
            raise ValueError('second device did not clear read notification')
        def privacy_barrier():
            fence = str(uuid.uuid4())
            items.append(fence)
            sql(f"BEGIN; SET LOCAL application_name='agent:m5-demo'; INSERT INTO porchlight.items(id,household_id,owner_id,title,kind,visibility) VALUES ('{fence}','{household}','{recipient}','M5 stream barrier','todo','household'); COMMIT;")
            read_sse_until(other_stream, 'item.changed', fence, forbid_notifications=True)
        privacy_barrier()
        print('PASS: member-private mark-read dismissal on both recipient devices, absent on other member', flush=True)
        # Quiet interval ends at the next local minute, allowing a bounded real-clock release proof.
        local = dt.datetime.now(dt.timezone.utc).astimezone(zone)
        end = (local + dt.timedelta(minutes=2)).replace(second=0, microsecond=0)
        start = local - dt.timedelta(minutes=1)
        prefs.update(quiet_start=start.strftime('%H:%M:%S'), quiet_end=end.strftime('%H:%M:%S'))
        current = api('PUT', '/v1/me/notification-prefs', prefs, current['version'])
        key = str(uuid.uuid4())
        sql(f"BEGIN; SET LOCAL application_name='agent:m5-demo'; SELECT porchlight.pl_emit_notification_event('{household}','{recipient}','comment','item','{items[2]}','{{}}','{items[2]}',NULL,'{key}'); COMMIT;")
        time.sleep(2)
        if any(n.get('item_id') == items[2] for n in api('GET', '/v1/notifications')['items']):
            raise ValueError('quiet notification released early')
        print('PASS: quiet hours hold non-reminder alerts', flush=True)
        digest = wait_for(lambda rows: next((n for n in rows if n['category'] == 'digest' and n.get('item_id') == items[2]), None), 130)
        if digest['count'] != 1 or not digest['released_at']:
            raise ValueError('digest mismatch')
        print('PASS: one morning digest released at quiet-hours end', flush=True)
        all_read = api('POST', '/v1/notifications/read-all', {})
        if all_read['count'] < 1:
            raise ValueError('read-all did not clear digest')
        read_sse_until(first_stream, 'notification.dismissed', digest['id'])
        read_sse_until(second_stream, 'notification.dismissed', digest['id'])
        privacy_barrier()
        print('PASS: member-private read-all dismissal on both devices, absent on other member', flush=True)
    finally:
        for conn, reply in streams:
            reply.close()
            conn.close()
        # Restore canonical preferences and hide only the three demo items. Audit remains intentional history.
        saved = api('GET', '/v1/me/notification-prefs')
        api('PUT', '/v1/me/notification-prefs', initial['prefs'], saved['version'])
        ids = ','.join(f"'{i}'" for i in items)
        sql(f"BEGIN; SET LOCAL application_name='agent:m5-demo'; UPDATE porchlight.items SET deleted_at=now() WHERE id IN ({ids}); COMMIT;")

    return cfg

if __name__ == '__main__':
    try:
        cfg = run(sys.argv[1])
        # Private config supplies paths; credentials stay in files / environment.
        stream_env = dict(os.environ)
        stream_env.update(M5_STREAM_BASE_URL=cfg['base_url'],
                          M5_STREAM_DATABASE_URL=os.environ['PORCHLIGHT_SUPERUSER_URL'],
                          M5_STREAM_ADMIN_CLIENT=cfg.get('stream_admin_device_dir', cfg['device_dir']),
                          M5_STREAM_READER_CLIENT=cfg.get('stream_reader_device_dir', cfg['other_member_device_dir']))
        subprocess.run(['bash', str(ROOT / 'deploy/scripts/m5-demo-stream.sh')], env=stream_env, check=True)
    except Exception:
        # Exception text may contain a URL, SQL row, credentials or response details. Never print it.
        print('FAIL: M5 provisioned-stack notification gate', file=sys.stderr)
        sys.exit(1)
