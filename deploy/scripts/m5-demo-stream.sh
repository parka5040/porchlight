#!/usr/bin/env bash
# Controller-only, LAPTOP DEV STACK ONLY. Never production: creates/soft-deletes a
# demo item and temporarily seeds aged outbox rows to exercise cursor expiration.
# The 8-day row expires all households' cursors until cleanup; isolated DEV DB only.
# Chain this from S6a's m5-demo.sh. Requires the SAME database as the TLS API.
# Configure privately (do not paste values into gate logs):
# M5_STREAM_BASE_URL          HTTPS LAN endpoint (certificate name must match)
# M5_STREAM_DATABASE_URL      SUPERUSER dev DB connection (INSERT/DELETE outbox rows)
# M5_STREAM_ADMIN_CLIENT      var/... directory: ca.crt, client.crt, client.key, session.token
# M5_STREAM_READER_CLIENT     same files, DIFFERENT live member in the same household
# Stdout: PASS/FAIL only. Private work dirs (0700) are retained on failure; inspect locally, never paste.
umask 022
set -Eeuo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
M5_STREAM_ROOT="$root" python3 -I - <<'PY'
import json, os, pathlib, shutil, subprocess, tempfile, time, traceback, urllib.parse, uuid
os.umask(0o077)
root = pathlib.Path(os.environ['M5_STREAM_ROOT']).resolve()
private = root / 'var'
private.mkdir(exist_ok=True)
work = pathlib.Path(tempfile.mkdtemp(prefix='m5-stream-', dir=private))
empty_ca = work / 'empty-ca'
empty_ca.mkdir()
log = (work / 'diagnostics.log').open('w')
env = os.environ.copy()
for name in ['SSLKEYLOGFILE','SSL_CERT_DIR','SSL_CERT_FILE','CURL_CA_BUNDLE','CURL_HOME']:
    env.pop(name, None)
serial = 0
item_id = None
sentinels = []
admin = None
base = None

def sql(statement):
    e = env.copy()
    # libpq does NOT expand URLs from PGDATABASE. Convert the URL privately into
    # a service file; neither credentials nor conninfo ever appear on argv.
    u = urllib.parse.urlsplit(os.environ['M5_STREAM_DATABASE_URL'])
    assert u.scheme in ('postgres', 'postgresql') and u.hostname, 'Postgres URL required'
    options = dict(urllib.parse.parse_qsl(u.query, keep_blank_values=True))
    options.update(dbname=urllib.parse.unquote(u.path.lstrip('/')))
    options.setdefault('host', u.hostname)
    if u.port is not None: options.setdefault('port', str(u.port))
    if u.username is not None: options.setdefault('user', urllib.parse.unquote(u.username))
    if u.password is not None: options.setdefault('password', urllib.parse.unquote(u.password))
    assert options['dbname'], 'Database name required'
    assert all(k.replace('_','').isalnum() and '\n' not in v and '\r' not in v and '\x00' not in v and v.strip() == v for k,v in options.items()), 'Invalid service option'
    service = work / 'pg_service.conf'
    service.write_text('[stream]\n' + ''.join(k + '=' + v + '\n' for k,v in options.items()))
    service.chmod(0o600)
    # Remove inherited connection defaults so this is the ONLY authority.
    for key in list(e):
        if key.startswith('PG'): e.pop(key)
    e['PGSERVICEFILE'] = str(service)
    e['PGSERVICE'] = 'stream' 
    p = subprocess.run(['psql','-X','-q','-At','-v','ON_ERROR_STOP=1'], input=statement,
                       text=True, env=e, stdout=subprocess.PIPE, stderr=log, timeout=20)
    assert p.returncode == 0, 'SQL fixture operation failed'
    return p.stdout.strip()

def client(name):
    p = pathlib.Path(os.environ[name]).resolve()
    assert p.is_relative_to(private), 'Credentials must live under var'
    for filename in ['ca.crt','client.crt','client.key','session.token']:
        f = (p / filename).resolve()
        assert f.is_relative_to(private) and f.is_file(), 'Credential file missing'
    return p

def request(c, method, path, body=None, version=None, after=None, stream=False, ca=None):
    global serial
    serial += 1
    prefix = work / str(serial)
    token = (c / 'session.token').read_text().strip()
    assert '\n' not in token and '\r' not in token, 'Invalid session file'
    config = prefix.with_suffix('.curl')
    headers = ['Authorization: Bearer ' + token,
               'Porchlight-Client-Version: ' + (root / 'VERSION').read_text().strip()]
    if method != 'GET':
        headers += ['Idempotency-Key: ' + str(uuid.uuid4()), 'Content-Type: application/json']
    if version is not None: headers.append('If-Match: ' + str(version))
    if after is not None: headers.append('Last-Event-ID: ' + str(after))
    config.write_text(''.join('header = ' + json.dumps(h) + '\n' for h in headers))
    out = prefix.with_suffix('.body')
    response_headers = prefix.with_suffix('.headers')
    cmd = ['curl','-q','--config',str(config),'--silent','--show-error','--noproxy','*',
           '--tlsv1.3','--tls-max','1.3','--cacert',str(ca or c/'ca.crt'),
           '--capath',str(empty_ca),'--cert',str(c/'client.crt'),'--key',str(c/'client.key'),
           '--max-time','3' if stream else '15','--no-buffer','--dump-header',str(response_headers),
           '--output',str(out),'--request',method]
    if body is not None:
        data = prefix.with_suffix('.json')
        data.write_text(json.dumps(body))
        cmd += ['--data-binary','@'+str(data)]
    cmd += [base + path]
    try:
        p = subprocess.run(cmd, env=env, stdout=log, stderr=log, timeout=20)
    finally:
        config.unlink(missing_ok=True)
    if ca is not None:
        return p.returncode, None
    assert p.returncode in ([0,28] if stream else [0]), 'TLS request failed'
    status = [line.split()[1] for line in response_headers.read_text().splitlines() if line.startswith('HTTP/')][-1]
    assert status in ['200','201','204'], 'Unexpected HTTP status'
    text = out.read_text()
    if stream:
        frames = []
        for frame in text.replace('\r\n','\n').split('\n\n'):
            e = [line[6:] for line in frame.splitlines() if line.startswith('event:')]
            d = [line[5:].lstrip() for line in frame.splitlines() if line.startswith('data:')]
            if e: frames.append((e[0].strip(), json.loads('\n'.join(d))))
        return frames
    return json.loads(text) if text else None

def projected(event_id):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        seq = sql("SELECT stream_seq FROM porchlight.outbox WHERE id='"+str(uuid.UUID(event_id))+"'")
        if seq: return int(seq)
        time.sleep(.05)
    raise AssertionError('Projector did not publish fixture')

rc = 0
try:
    for tool in ['curl','psql','openssl']:
        assert shutil.which(tool), 'Required controller tool missing'
    base = os.environ['M5_STREAM_BASE_URL'].rstrip('/')
    u = urllib.parse.urlsplit(base)
    assert u.scheme == 'https' and u.hostname and not u.username and not u.password and not u.query and not u.fragment and u.path in ['', '/'], 'HTTPS base endpoint required'
    admin = client('M5_STREAM_ADMIN_CLIENT')
    reader = client('M5_STREAM_READER_CLIENT')
    a = request(admin,'GET','/v1/me')
    r = request(reader,'GET','/v1/me')
    assert a['household_id'] == r['household_id'] and a['member_id'] != r['member_id'], 'Two household members required'
    assert a['role'] == 'super_admin', 'Admin client must be a super admin'
    household = str(uuid.UUID(a['household_id']))
    recipient = str(uuid.UUID(r['member_id']))
    assert sql("SELECT rolsuper FROM pg_roles WHERE rolname=current_user") == 't', 'Superuser DB connection required'
    # Verify the private DB is the same household as the pinned TLS API before writes.
    assert sql("SELECT count(*) FROM porchlight.households WHERE id='"+household+"'") == '1'
    subprocess.run(['openssl','req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:prime256v1',
                    '-nodes','-subj','/CN=M5WrongRoot','-days','1','-keyout',str(work/'wrong.key'),
                    '-out',str(work/'wrong.crt')], stdout=log, stderr=log, check=True)
    wrong, _ = request(reader,'GET','/v1/me',ca=work/'wrong.crt')
    assert wrong == 60, 'Wrong-root verification must fail'
    print('PASS: stream pinned TLS and wrong-root rejection')
    cursor = request(reader,'GET','/v1/snapshot')['cursor']
    created = request(admin,'POST','/v1/items',{'title':'M5 stream demo fixture','visibility':'household'})
    item_id = str(uuid.UUID(created['id']))
    event_id = sql("SELECT id FROM porchlight.outbox WHERE entity_id='"+item_id+"' AND kind='item.changed' ORDER BY id LIMIT 1")
    initial = projected(event_id)
    replay = request(reader,'GET','/v1/stream',after=cursor,stream=True)
    assert any(k=='item.changed' and d['entity_id']==item_id and d['seq']==initial for k,d in replay)
    assert all(set(d) <= {'seq','kind','entity_type','entity_id','evicted','cursor','reason'} for _,d in replay)
    print('PASS: stream replay by snapshot cursor and ID-only tickles')
    current = request(admin,'GET','/v1/items/'+item_id)
    request(admin,'PATCH','/v1/items/'+item_id,{'visibility':'private'},version=current['version'])
    access_id = sql("SELECT id FROM porchlight.outbox WHERE entity_id='"+item_id+"' AND kind='item.access_changed' ORDER BY id DESC LIMIT 1")
    access_seq = projected(access_id)
    eviction = request(reader,'GET','/v1/stream',after=initial,stream=True)
    assert any(k=='item.access_changed' and d['entity_id']==item_id and d.get('evicted') is True for k,d in eviction)
    current = request(admin,'GET','/v1/items/'+item_id)
    request(admin,'PATCH','/v1/items/'+item_id,{'title':'M5 hidden fixture changed'},version=current['version'])
    hidden_id = sql("SELECT id FROM porchlight.outbox WHERE entity_id='"+item_id+"' AND kind='item.changed' ORDER BY id DESC LIMIT 1")
    projected(hidden_id)
    hidden = request(reader,'GET','/v1/stream',after=access_seq,stream=True)
    assert not any(k in ['item.changed','derived','occurrence.changed'] and d['entity_id']==item_id for k,d in hidden)
    print('PASS: stream eviction and replay ACL after visibility loss')
    for age in ["now()-interval '8 days'", 'now()']:
        eid = sql("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id,payload,created_at) VALUES ('"+household+"','settings.changed','member_settings','"+recipient+"',jsonb_build_object('member_id','"+recipient+"'),"+age+") RETURNING id")
        sentinels.append(str(uuid.UUID(eid)))
        sql("SELECT pg_notify('porchlight_outbox','')")
    expired = projected(sentinels[0])
    fresh = projected(sentinels[1])
    assert fresh > expired
    resnapshot = request(reader,'GET','/v1/stream',after=expired,stream=True)
    assert resnapshot == [('resnapshot',{'reason':'cursor_expired'})]
    print('PASS: stream expired cursor resnapshot and clean EOF')
except Exception:
    traceback.print_exc(file=log)
    print('FAIL: stream TLS gate')
    rc = 1
finally:
    try:
        for eid in sentinels:
            sql("DELETE FROM porchlight.outbox WHERE id='"+eid+"'")
        if item_id is not None:
            current = request(admin,'GET','/v1/items/'+item_id)
            request(admin,'DELETE','/v1/items/'+item_id,body={},version=current['version'])  # every mutation carries a JSON body
    except Exception:
        traceback.print_exc(file=log)
        print('FAIL: stream fixture cleanup')
        rc = 1
    log.close()
    if rc == 0:
        shutil.rmtree(work)
    else:
        print("FAIL: private diagnostics retained in " + str(work))
raise SystemExit(rc)
PY
