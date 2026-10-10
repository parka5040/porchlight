#!/usr/bin/env python3
"""Controller-only live client driver. Private config, snapshots and logs stay under var.
No credential arguments, no arbitrary exception/reply output, no device/profile substitution.
"""
import datetime as dt
import http.client
import io
import json
import os
import re
from pathlib import Path
import shutil
import signal
import sqlite3
import fcntl
import ssl
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.parse
import uuid
import xml.etree.ElementTree as ET
from zoneinfo import ZoneInfo

ROOT = Path(__file__).resolve().parents[2]
VAR = (ROOT / 'var').resolve()
APP = 'app.porchlight'


def preference_inventory(tree, kind):
    table = next((node.text for node in tree if node.attrib.get('name') == 'table'), None)
    if table:
        value = json.loads(table)
        return value if isinstance(value, list) else value.get('tickets', [])
    if kind == 'alarms':
        return [json.loads(node.text) for node in tree if node.attrib.get('name', '').startswith('ticket:')]
    return []


def posted_action_key(snapshot, notification_id):
    row = next((r for r in snapshot['delivery'] if r['key'] == notification_id), None)
    if not row or not row.get('posted') or row.get('dismissed') or not row.get('actionKey'):
        raise ValueError('device has not delivered the original actionable alert')
    return row['actionKey']


def private(value):
    path = Path(value).resolve()
    if not path.is_relative_to(VAR):
        raise ValueError('runtime path outside var')
    return path


def wait_for(predicate, seconds=90):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            value = predicate()
            if value:
                return value
        except (sqlite3.Error, FileNotFoundError, ET.ParseError, json.JSONDecodeError):
            pass  # A copied WAL may race its writer; never accept a partial observation.
        time.sleep(1)
    raise ValueError('observation deadline')


class Gate:
    def __init__(self, mode, config, serial, keep):
        self.mode, self.serial, self.keep = mode, serial, keep
        self.cfg = json.loads(private(config).read_text())
        if self.cfg.get('disposable') is not True or not serial.startswith('emulator-'):
            raise ValueError('disposable clients required')
        self.desktop_home = private(self.cfg['desktop_data_home'])
        self.desktop_dir = self.desktop_home / 'porchlight'
        if not (self.desktop_dir / 'device.p12').is_file():
            raise ValueError('pre-enrolled disposable desktop required')
        self.run = Path(tempfile.mkdtemp(prefix='m5-client-', dir=VAR / 'gates'))
        self.log = (self.run / 'driver.log').open('wb')
        self.desktop = None
        self.items = []
        self.initial_prefs = None
        self.old_zone = None
        self.start = time.monotonic()
        self.endpoint = urllib.parse.urlsplit(self.cfg['base_url'])
        if self.endpoint.scheme != 'https' or self.endpoint.hostname not in ('localhost', '127.0.0.1', 'porchlight.lan'):
            raise ValueError('local laptop HTTPS stack required')
        self.household = str(uuid.UUID(self.cfg['household_id']))
        self.member = str(uuid.UUID(self.cfg['recipient_member_id']))
        device = private(self.cfg['device_dir'])
        self.ctx = ssl.create_default_context(cafile=str(device / 'ca.crt'))
        self.ctx.load_cert_chain(str(device / 'client.crt'), str(device / 'client.key'))
        self.token = (device / 'session.token').read_text().strip()

    def command(self, args, data=None, capture=False, timeout=60, env=None):
        result = subprocess.run(args, input=data, stdout=subprocess.PIPE if capture else self.log,
                                stderr=self.log, timeout=timeout, env=env)
        if result.returncode:
            raise ValueError('controller command failed')
        return result.stdout if capture else b''

    def adb(self, *args, **kwargs):
        return self.command(['adb', '-s', self.serial, *args], **kwargs)

    def shell(self, *args):
        return self.adb('shell', *args, capture=True).decode().strip()

    def instrument(self, name, expected='granted', notifications=None):
        # R66: USE_EXACT_ALARM auto-grants API33+. Test real denial ONLY on API31-32.
        api = int(self.shell('getprop', 'ro.build.version.sdk'))
        if api >= 33:
            if expected == 'denied':
                self.log.write(f'R66: API {api} exact-alarm denied scenario overridden to granted (USE_EXACT_ALARM)\n'.encode())
                self.log.flush()
            expected = 'granted'
        # Revocation kills the app: ALWAYS set exact-alarm app-op BEFORE instrumentation.
        self.shell('cmd', 'appops', 'set', APP, 'SCHEDULE_EXACT_ALARM', 'allow' if expected == 'granted' else 'deny')
        # The POST_NOTIFICATIONS state the caller set before this run; the test fails unless it starts there.
        if notifications not in (None, 'denied', 'granted'):
            raise ValueError('unknown notification state')
        declared = ('-e', 'C4_NOTIFICATIONS_EXPECTED', notifications) if notifications else ()
        output = self.adb('shell', 'am', 'instrument', '-w', '-e', 'class', 'app.porchlight.android.' + name,
                          '-e', 'C4_EXACT_EXPECTED', expected, *declared, APP + '.test/androidx.test.runner.AndroidJUnitRunner',
                          capture=True, timeout=180)
        self.log.write(output); self.log.flush()
        if b'OK (' not in output or b'FAILURES!!!' in output or b'INSTRUMENTATION_FAILED' in output:
            raise ValueError('instrumentation did not pass')

    def push_json(self, name, value):
        self.shell('run-as', APP, 'mkdir', '-p', 'files')
        # Command is fixed; credentials travel only on stdin to a 0600 private app file.
        if name not in ('m5-enroll', 'm5-dismiss'):
            raise ValueError('unknown private input')
        self.adb('shell', '-T', 'run-as app.porchlight sh -c "umask 077; cat > files/' + name + '.json"',
                 data=json.dumps(value).encode())

    def api(self, method, path, body=None, version=None):
        conn = http.client.HTTPSConnection(self.endpoint.hostname, self.endpoint.port or 443, context=self.ctx, timeout=20)
        headers = {'Authorization': 'Bearer ' + self.token, 'Porchlight-Client-Version': (ROOT / 'VERSION').read_text().strip()}
        if method != 'GET':
            headers.update({'Content-Type': 'application/json', 'Idempotency-Key': str(uuid.uuid4())})
        if version is not None:
            headers['If-Match'] = str(version)
        try:
            conn.request(method, self.endpoint.path.rstrip('/') + path, json.dumps(body) if body is not None else None, headers)
            reply = conn.getresponse()
            raw = reply.read(4 * 1024 * 1024)
            if reply.status not in (200, 201, 204):
                raise ValueError('unexpected API status')
            return json.loads(raw) if raw else None
        finally:
            conn.close()

    def sql(self, statement):
        db = urllib.parse.urlsplit(os.environ['PORCHLIGHT_SUPERUSER_URL'])
        pg = dict(os.environ)
        pg.update(PGHOST=urllib.parse.parse_qs(db.query).get('host', [db.hostname or ''])[0],
                  PGPORT=str(db.port or 5432), PGDATABASE=urllib.parse.unquote(db.path.lstrip('/')),
                  PGUSER='porchlight_agent', PGPASSWORD=os.environ['PORCHLIGHT_AGENT_PASSWORD'])
        self.command(['psql', '-X', '-q', '-v', 'ON_ERROR_STOP=1'], data=statement.encode(), env=pg)

    def android_snapshot(self):
        # Do NOT extract an untrusted archive. Accept only fixed runtime filenames into a new empty directory.
        raw = self.adb('exec-out', 'run-as', APP, 'tar', '-cf', '-', 'databases', 'shared_prefs', capture=True)
        directory = Path(tempfile.mkdtemp(prefix='snapshot-', dir=self.run))
        result = {'items': [], 'notifications': [], 'delivery': [], 'alarms': []}
        try:
            with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
                for member in archive.getmembers():
                    if not member.isfile() or member.size > 32 * 1024 * 1024:
                        continue
                    name = member.name.removeprefix('./')
                    if name in ('databases/read-cache.db', 'databases/read-cache.db-wal', 'databases/read-cache.db-shm'):
                        (directory / Path(name).name).write_bytes(archive.extractfile(member).read())
                    if name in ('shared_prefs/os-delivery.xml', 'shared_prefs/alarm-ledger.xml'):
                        tree = ET.fromstring(archive.extractfile(member).read())
                        kind = 'delivery' if 'os-delivery' in name else 'alarms'
                        result[kind] = preference_inventory(tree, kind)
            result.update(self.cache(directory / 'read-cache.db'))
            return result
        finally:
            shutil.rmtree(directory)

    @staticmethod
    def cache(path):
        with sqlite3.connect('file:' + str(path) + '?mode=ro', uri=True) as db:
            return {'items': [json.loads(row[0]) for row in db.execute('SELECT payload FROM CacheItem')],
                    'notifications': [json.loads(row[0]) for row in db.execute('SELECT payload FROM CacheNotification')]}

    def desktop_snapshot(self):
        result = self.cache(self.desktop_dir / 'read-cache.db')
        result['delivery'] = json.loads((self.desktop_dir / 'os-delivery.json').read_text()).get('records', [])
        result['alarms'] = [json.loads(path.read_text()) for path in (self.desktop_dir / 'alarm-tickets').glob('*.json')]
        return result

    def both(self, predicate):
        return predicate(self.android_snapshot()) and predicate(self.desktop_snapshot())

    def start_activity(self):
        self.shell('am', 'start', '-W', '-n', APP + '/app.porchlight.android.MainActivity')

    def network(self, enabled):
        self.shell('svc', 'wifi', 'enable' if enabled else 'disable')
        self.shell('svc', 'data', 'enable' if enabled else 'disable')

    def alarm_dump(self, ticket):
        marker = f"app.porchlight.REMINDER.{ticket['code']}.{ticket['generation']}"
        if marker not in self.shell('dumpsys', 'alarm'):
            raise ValueError('scheduled alarm missing from OS dump')

    def assigned(self):
        item = str(uuid.uuid4()); self.items.append(item)
        self.sql(f"BEGIN; SET LOCAL application_name='agent:m5-client'; INSERT INTO porchlight.items(id,household_id,owner_id,title,kind,visibility) VALUES ('{item}','{self.household}','{self.member}','M5 client assignment','todo','private'); INSERT INTO porchlight.item_assignees(household_id,item_id,member_id) VALUES ('{self.household}','{item}','{self.member}'); COMMIT;")
        return item

    def prefs(self, value):
        current = self.api('GET', '/v1/me/notification-prefs')
        return self.api('PUT', '/v1/me/notification-prefs', value, current['version'])

    def witness(self, phase):
        # Visible Compose/niri behavior is a human gate, not inferred from a successful database poll.
        # Fresh per-run directory prevents stale attestations. No prompt, credential, or UI dump on stdout.
        witness = self.run / (phase + '.json')
        desktop_image = self.run / ('desktop-' + phase + '.png')
        print('PASS: ' + phase + ' data ready; controller must inspect BOTH visible clients and write evidence under ' + str(self.run.relative_to(ROOT)), flush=True)
        def observed():
            if not witness.exists() or not desktop_image.exists():
                return False
            claim = json.loads(witness.read_text())
            return claim == {'android': True, 'desktop': True} and desktop_image.read_bytes().startswith(b'\x89PNG\r\n\x1a\n')
        wait_for(observed, 300)
        (self.run / ('android-' + phase + '.png')).write_bytes(self.adb('exec-out', 'screencap', '-p', capture=True))
        print('PASS: controller-attested visible ' + phase + ' on Android and desktop (paired evidence saved)', flush=True)

    def platform_checks(self):
        # API33+ notification enablement IS the POST_NOTIFICATIONS grant, and `pm revoke` kills the app, so each run's
        # starting grant is set here BEFORE instrumentation and declared to the test, which asserts it. Both runs cover
        # denial -> recovery: run 1 from the cold denied start, run 2 by an in-test no-kill revocation of the grant.
        self.shell('pm', 'revoke', APP, 'android.permission.POST_NOTIFICATIONS')
        self.instrument('C4PlatformControllerTest', 'denied', notifications='denied')
        self.shell('pm', 'grant', APP, 'android.permission.POST_NOTIFICATIONS')
        self.instrument('C4PlatformControllerTest', 'granted', notifications='granted')
        print('PASS: denied/granted notifications, R66 per-API exact alarms, dumps, durable actions', flush=True)

    def execute(self):
        self.adb('install', '-r', str(ROOT / 'client/androidApp/build/outputs/apk/debug/androidApp-debug.apk'))
        self.adb('install', '-r', str(ROOT / 'client/androidApp/build/outputs/apk/androidTest/debug/androidApp-debug-androidTest.apk'))
        self.platform_checks()
        self.push_json('m5-enroll', self.cfg['android_enrollment'])
        self.instrument('C4GateControllerTest')
        self.start_activity()
        launcher = ROOT / 'client/desktopApp/build/compose/binaries/main/app/porchlight/bin/porchlight'
        env = dict(os.environ, XDG_DATA_HOME=str(self.desktop_home), XDG_CONFIG_HOME=str(self.run / 'desktop-config'))
        with (self.desktop_dir / 'instance.lock').open('a') as lock:
            fcntl.lockf(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)  # Fail rather than observe somebody else's desktop.
            fcntl.lockf(lock, fcntl.LOCK_UN)
        self.desktop = subprocess.Popen([str(launcher)], stdout=self.log, stderr=self.log, env=env)
        self.start = time.monotonic()
        initial = self.api('GET', '/v1/me/notification-prefs')
        if initial['member_id'] != self.member:
            raise ValueError('recipient mismatch')
        self.initial_prefs = dict(initial['prefs'])
        prefs = dict(self.initial_prefs, assignments=True, activity=True, reminders=True, digest=True, quiet_start=None, quiet_end=None)
        self.prefs(prefs)
        # Real server derivation/reminder -> both caches -> both local timer ledgers -> OS delivery.
        due = dt.datetime.now(dt.timezone.utc) + dt.timedelta(hours=1)
        timed = self.api('POST', '/v1/items', {'title': 'M5 offline alarm', 'kind': 'deadline', 'time_mode': 'fixed',
            'tzid': 'UTC', 'due_at': due.isoformat(), 'assignees': [self.member], 'reminder_offsets': [0]})
        self.items.append(timed['id'])
        wait_for(lambda: self.both(lambda s: any(t.get('itemId') == timed['id'] and t.get('active', True) for t in s['alarms'])))
        ticket = next(t for t in self.android_snapshot()['alarms'] if t.get('itemId') == timed['id'])
        self.alarm_dump(ticket)
        updated = self.api('PATCH', '/v1/items/' + timed['id'], {'title': 'M5 live update'}, timed['version'])
        wait_for(lambda: self.both(lambda s: any(i['id'] == timed['id'] and i['title'] == 'M5 live update' for i in s['items'])))
        print('PASS: dev-stack update reached Android and desktop live caches', flush=True)
        self.witness('visible-update')
        due = dt.datetime.now(dt.timezone.utc) + dt.timedelta(seconds=150)
        self.api('PATCH', '/v1/items/' + timed['id'], {'due_at': due.isoformat()}, updated['version'])
        target = due.timestamp()
        wait_for(lambda: self.both(lambda s: any(t.get('itemId') == timed['id'] and abs(dt.datetime.fromisoformat(t['fireAt'].replace('Z', '+00:00')).timestamp() - target) < 1 for t in s['alarms'])))
        ticket = next(t for t in self.android_snapshot()['alarms'] if t.get('itemId') == timed['id'])
        self.alarm_dump(ticket)
        self.network(False)
        self.shell('dumpsys', 'battery', 'unplug')
        self.shell('cmd', 'deviceidle', 'whitelist', '-' + APP)
        if APP in self.shell('cmd', 'deviceidle', 'whitelist'):
            raise ValueError('battery optimization was bypassed')
        self.shell('input', 'keyevent', '223')
        self.shell('cmd', 'deviceidle', 'enable')
        self.shell('cmd', 'deviceidle', 'force-idle')
        if 'IDLE' not in self.shell('cmd', 'deviceidle', 'get', 'deep'):
            raise ValueError('device did not enter doze')
        wait_for(lambda: self.both(lambda s: any(r.get('localAlarm') and r.get('itemId') == timed['id'] and r.get('posted') for r in s['delivery'])), 240)
        # Check OS, not merely the ledger, on the offline device.
        dump = self.shell('dumpsys', 'notification', '--noredact')
        if not any('pkg=app.porchlight' in line and 'tag=alarm' in line and re.search(r'\bid=' + str(ticket['code']) + r'\b', line) for line in dump.splitlines()):
            raise ValueError('offline alarm absent from OS notifications')
        print('PASS: offline real alarm delivered in forced idle with battery optimization on', flush=True)
        self.shell('cmd', 'deviceidle', 'unforce'); self.shell('input', 'keyevent', '224')
        self.network(True)
        assigned = self.assigned()
        row = wait_for(lambda: next((n for n in self.api('GET', '/v1/notifications?limit=200')['items'] if n.get('item_id') == assigned), None))
        wait_for(lambda: self.both(lambda s: any(r['key'] == row['id'] and r.get('posted') for r in s['delivery'])))
        self.dismiss_on_android(row['id'])
        if not any(n['id'] == row['id'] and n.get('read_at') for n in self.api('GET', '/v1/notifications?limit=200')['items']):
            raise ValueError('dismissal not committed')
        print('PASS: Android original-key dismissal propagated to desktop OS inventory', flush=True)
        self.witness('visible-dismissal')
        members = self.api('GET', '/v1/members')['items']
        zone = ZoneInfo(next(m['tzid'] for m in members if m['id'] == self.member))
        now = dt.datetime.now(zone)
        prefs.update(quiet_start=(now - dt.timedelta(minutes=1)).strftime('%H:%M:%S'),
                     quiet_end=(now + dt.timedelta(minutes=2)).strftime('%H:%M:%S'))
        self.prefs(prefs); quiet_item = self.assigned()
        time.sleep(5)
        if any(n.get('item_id') == quiet_item for n in self.api('GET', '/v1/notifications')['items']):
            raise ValueError('quiet hours released early')
        digest = wait_for(lambda: next((n for n in self.api('GET', '/v1/notifications?limit=200')['items'] if n.get('item_id') == quiet_item and n['category'] == 'digest'), None), 150)
        wait_for(lambda: self.both(lambda s: any(r['key'] == digest['id'] and r.get('posted') for r in s['delivery'])))
        print('PASS: quiet-hours digest released and delivered on both clients', flush=True)
        self.witness('visible-digest')
        if self.mode == 'soak':
            self.soak()
        if self.desktop.poll() is not None:
            raise ValueError('desktop exited')

    def dismiss_on_android(self, notification_id):
        self.push_json('m5-dismiss', {'id': notification_id})
        posted_action_key(self.android_snapshot(), notification_id)
        self.instrument('C4GateDismissControllerTest')
        self.shell('wm', 'dismiss-keyguard')
        self.start_activity()  # instrumentation force-stops the target/FGS; restore the fixture
        wait_for(lambda: self.both(lambda s: all(r['key'] != notification_id for r in s['delivery'])))

    def soak(self):
        # A future alarm remains through real broadcasts, time/zone changes and reboot.
        due = dt.datetime.now(dt.timezone.utc) + dt.timedelta(hours=2)
        item = self.api('POST', '/v1/items', {'title': 'M5 reboot reminder', 'kind': 'deadline', 'time_mode': 'fixed',
            'tzid': 'UTC', 'due_at': due.isoformat(), 'assignees': [self.member], 'reminder_offsets': [0]})
        self.items.append(item['id'])
        ticket = wait_for(lambda: next((t for t in self.android_snapshot()['alarms'] if t.get('itemId') == item['id']), None))
        self.alarm_dump(ticket)
        self.old_zone = self.shell('getprop', 'persist.sys.timezone')
        self.shell('settings', 'put', 'global', 'auto_time', '0')
        self.shell('settings', 'put', 'global', 'auto_time_zone', '0')
        self.shell('cmd', 'alarm', 'set-timezone', 'Pacific/Auckland')
        wait_for(lambda: self.shell('getprop', 'persist.sys.timezone') == 'Pacific/Auckland')
        self.shell('cmd', 'alarm', 'set-time', str(int(time.time() * 1000) + 120_000))
        if abs(int(self.shell('date', '+%s')) - int(time.time()) - 120) > 10:
            raise ValueError('time change not applied')
        self.alarm_dump(ticket)
        self.shell('cmd', 'alarm', 'set-time', str(int(time.time() * 1000)))
        self.adb('reboot')
        wait_for(self.booted, 240)
        self.shell('input', 'keyevent', '82')  # disposable AVD: dismiss unsecured lockscreen after reboot
        wait_for(lambda: self.alarm_present(ticket))  # BOOT receiver must repair alarms BEFORE Activity startup.
        current = self.api('GET', '/v1/items/' + item['id'])
        self.api('PATCH', '/v1/items/' + item['id'], {'title': 'M5 boot stream recovery'}, current['version'])
        wait_for(lambda: self.both(lambda state: any(i['id'] == item['id'] and i['title'] == 'M5 boot stream recovery' for i in state['items'])))
        print('PASS: time/zone broadcasts and reboot retained the real scheduled alarm', flush=True)
        # Keep both clients alive for at least 30 monotonic minutes, exercising repeated network loss/recovery.
        iteration = 0
        while time.monotonic() - self.start < 1800:
            iteration += 1
            self.network(False)
            current = self.api('GET', '/v1/items/' + item['id'])
            title = 'M5 network recovery ' + str(iteration)
            self.api('PATCH', '/v1/items/' + item['id'], {'title': title}, current['version'])
            time.sleep(10); self.network(True)
            wait_for(lambda: self.both(lambda s: any(i['id'] == item['id'] and i['title'] == title for i in s['items'])))
            self.alarm_dump(ticket)
            if self.desktop.poll() is not None:
                raise ValueError('desktop exited during soak')
            time.sleep(min(60, max(0, 1800 - (time.monotonic() - self.start))))
        print('PASS: fresh-AVD 30-minute monotonic Android + desktop soak', flush=True)

    def booted(self):
        result = subprocess.run(['adb', '-s', self.serial, 'shell', 'getprop', 'sys.boot_completed'],
                                stdout=subprocess.PIPE, stderr=self.log, timeout=10)
        return result.returncode == 0 and result.stdout.strip() == b'1'

    def alarm_present(self, ticket):
        marker = f"app.porchlight.REMINDER.{ticket['code']}.{ticket['generation']}"
        return marker in self.shell('dumpsys', 'alarm')

    def cleanup(self):
        # Best-effort restoration, but restoration errors make the gate fail (never hide them).
        failed = False
        for action in (lambda: self.network(True), lambda: self.shell('cmd', 'deviceidle', 'unforce'),
                       lambda: self.shell('dumpsys', 'battery', 'reset'),
                       lambda: self.shell('cmd', 'alarm', 'set-time', str(int(time.time() * 1000))),
                       lambda: self.shell('settings', 'put', 'global', 'auto_time', '1'),
                       lambda: self.shell('settings', 'put', 'global', 'auto_time_zone', '1')):
            try:
                action()
            except Exception:
                failed = True
        if self.old_zone:
            try:
                self.shell('cmd', 'alarm', 'set-timezone', self.old_zone)
            except Exception:
                failed = True
        for name in ('m5-enroll', 'm5-dismiss'):
            try:
                self.shell('run-as', APP, 'rm', '-f', 'files/' + name + '.json')
            except Exception:
                failed = True
        if self.initial_prefs is not None:
            try:
                self.prefs(self.initial_prefs)
            except Exception:
                failed = True
        if not self.keep:
            for item in self.items:
                try:
                    current = self.api('GET', '/v1/items/' + item)
                    self.api('DELETE', '/v1/items/' + item, {}, current['version'])
                except Exception:
                    failed = True
        if self.desktop is not None:
            self.desktop.terminate()
            try:
                self.desktop.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.desktop.kill(); self.desktop.wait()
        self.log.close()
        if failed:
            raise ValueError('gate cleanup incomplete')


def main():
    os.umask(0o077)
    def interrupted(signum, frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    gate = None
    failed = False
    try:
        mode, config, serial, keep = sys.argv[1:]
        if mode not in ('demo', 'soak') or keep not in ('0', '1'):
            raise ValueError('invalid controller arguments')
        gate = Gate(mode, config, serial, keep == '1')
        gate.execute()
    except BaseException as error:
        failed = True
        if gate is not None:
            gate.log.write(('FAIL: driver exception class ' + type(error).__name__ + '\n').encode()); gate.log.flush()
    finally:
        if gate is not None:
            try:
                gate.cleanup()
            except BaseException:
                failed = True
    if failed:
        print('FAIL: M5 client gate or cleanup (private diagnostics under var/gates)', flush=True)
        return 1
    print('PASS: M5 ' + mode + ' client checks, controller-visible evidence and cleanup complete', flush=True)
    return 0


if __name__ == '__main__':
    sys.exit(main())
