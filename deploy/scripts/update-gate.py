#!/usr/bin/env python3
"""Private controller helpers for update-acceptance.sh; no production endpoints or credentials in argv."""
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import shutil
import socket
import ssl
import subprocess
import sys
import time
import tomllib
import urllib.parse
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
VERSIONS = ['9.9.8', '9.9.9', '9.9.10']
APP = 'app.porchlight'


def digest(path):
    with path.open('rb') as stream:
        return {'size': path.stat().st_size, 'sha256': hashlib.file_digest(stream, 'sha256').hexdigest()}


def member_path(root, name):
    parts = name.split('/')
    if not parts or any(not re.fullmatch(r'[A-Za-z0-9_][A-Za-z0-9._-]*', part) or
                        part in ('.', '..', '__pycache__') or part.endswith('.pyc') for part in parts):
        raise ValueError('unsafe member')
    path = root
    for part in parts:
        path = path / part
        if path.is_symlink():
            raise ValueError('symlink member')
    return path


def check_file(path, expected):
    if path.is_symlink() or not path.is_file() or digest(path) != expected:
        raise ValueError('artifact checksum or size mismatch')


def bump(copy, version):
    if version not in VERSIONS:
        raise ValueError('gate version required')
    cargo = copy / 'server/Cargo.toml'
    old = tomllib.loads(cargo.read_text())['workspace']['package']['version']
    text, count = cargo.read_text(), 0
    # Bound the substitution to the workspace.package table, not dependency versions.
    tables = re.split(r'(?=^\[)', text, flags=re.M)
    for index, table in enumerate(tables):
        if table.startswith('[workspace.package]'):
            tables[index], count = re.subn(r'^version = "' + re.escape(old) + r'"$',
                                          'version = "' + version + '"', table, flags=re.M)
    if count != 1:
        raise ValueError('workspace version not found')
    cargo.write_text(''.join(tables))
    lock = copy / 'server/Cargo.lock'
    packages = lock.read_text().split('[[package]]')
    changed = 0
    for index, package in enumerate(packages[1:], 1):
        if re.search(r'^name = "porchlight-(?:api|cli|core|db|pki)"$', package, re.M):
            packages[index], count = re.subn(r'^version = "' + re.escape(old) + r'"$',
                                             'version = "' + version + '"', package, count=1, flags=re.M)
            if count != 1:
                raise ValueError('workspace lock version mismatch')
            changed += count
    if not changed:
        raise ValueError('workspace lock entries missing')
    lock.write_text('[[package]]'.join(packages))
    (copy / 'VERSION').write_text(version + '\n')
    if version == '9.9.10':
        migrations = copy / 'server/db/migrations'
        number = max(int(p.name.split('_')[0]) for p in migrations.glob('*.sql')) + 1
        with (migrations / f'{number:014d}_update_gate_forced_failure.sql').open('x') as stream:
            stream.write("DO $$ BEGIN RAISE EXCEPTION 'update gate: forced migration failure'; END $$;\n")


def inventory(base):
    result = {}
    for area in ('dists', 'android'):
        directory = base / area
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError('missing build artifacts')
        for path in sorted(directory.rglob('*')):
            name = path.relative_to(base).as_posix()
            member_path(base, name)
            if path.is_file():
                result[name] = digest(path)
            elif not path.is_dir():
                raise ValueError('non-regular build artifact')
    return result


def record_build(base, public_path, signer_path):
    public = public_path.read_text().splitlines()[-1]
    signer = signer_path.read_text().strip()
    if not re.fullmatch('[0-9a-f]{64}', signer):
        raise ValueError('invalid debug signer')
    record = {'versions': VERSIONS, 'minisign_public_key': public, 'debug_signer_sha256': signer,
              'artifacts': inventory(base)}
    (base / 'build.json').write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')
    verify_build(base)


def verify_build(base):
    record = json.loads((base / 'build.json').read_text())
    if record['versions'] != VERSIONS or inventory(base) != record['artifacts']:
        raise ValueError('build inventory changed')
    for name in ('androidApp-debug.apk', 'androidApp-debug-androidTest.apk'):
        if 'android/' + name not in record['artifacts']:
            raise ValueError('instrumentation build missing')
    for version in VERSIONS:
        source = base / 'dists' / version
        subprocess.run(['minisign', '-Vm', str(source / 'manifest.json'), '-x', str(source / 'manifest.json.minisig'),
                        '-P', record['minisign_public_key']], check=True, stdout=subprocess.DEVNULL)
        manifest = json.loads((source / 'manifest.json').read_text())
        if manifest['version'] != version:
            raise ValueError('wrong dist version')
        for name, expected in manifest['files'].items():
            check_file(member_path(source, name), expected)
        release = json.loads((source / 'release.json').read_text())
        if release['android']['signer_sha256'] != [record['debug_signer_sha256']] or release['version'] != version:
            raise ValueError('wrong release signer or version')


def mirror(base, directory):
    directory.mkdir(mode=0o700)
    for version in VERSIONS:
        source = base / 'dists' / version
        target = directory / 'download' / ('v' + version)
        target.mkdir(mode=0o700, parents=True)
        names = sorted(set(json.loads((source / 'manifest.json').read_text())['files']) |
                       {'manifest.json', 'manifest.json.minisig'})
        for name in names:
            path = member_path(source, name)
            if not path.is_file():
                raise ValueError('tar member not a regular file')
        # Same explicit NUL inventory and GNU tar flags as publish-release.sh.
        subprocess.run(['tar', '--format=gnu', '--sort=name', '--mtime=@0', '--owner=0', '--group=0',
                        '--numeric-owner', '--hard-dereference', '--no-recursion', '-cf',
                        str(target / f'porchlight-{version}-dist.tar'), '-C', str(source),
                        '--null', '--verbatim-files-from', '-T', '-'],
                       input=b''.join(name.encode() + b'\0' for name in names), check=True)
        for name in ('manifest.json', 'manifest.json.minisig', f'porchlight-{version}.apk', 'install.sh'):
            shutil.copy2(source / name, target / name)
    latest(directory, '9.9.9')


def latest(directory, version):
    if version not in VERSIONS:
        raise ValueError('gate version required')
    parent = directory / 'latest'
    parent.mkdir(mode=0o700, exist_ok=True)
    # Only this generated local symlink is served, never a link from a dist archive.
    link = parent / 'download.next'
    link.symlink_to('../download/v' + version, target_is_directory=True)
    link.replace(parent / 'download')


class Device:
    def __init__(self, work, label, port):
        self.work, self.port = work, int(port)
        self.device = work / 'devices' / label
        self.ctx = ssl.create_default_context(cafile=str(self.device / 'ca.crt'))
        self.ctx.load_cert_chain(str(self.device / 'client.crt'), str(self.device / 'client.key'))

    def get(self, path, version='9.9.9', session=False, headers=None, output=None):
        conn = http.client.HTTPSConnection('localhost', self.port, context=self.ctx, timeout=60)
        auth = {'Porchlight-Client-Version': version, **(headers or {})}
        if session:
            auth['Authorization'] = 'Bearer ' + (self.device / 'session.token').read_text().strip()
        try:
            conn.request('GET', path, headers=auth)
            response = conn.getresponse()
            if output is None:
                body = response.read(4 * 1024 * 1024 + 1)
                if len(body) > 4 * 1024 * 1024:
                    raise ValueError('oversize JSON response')
            else:
                with output.open('xb') as stream:
                    shutil.copyfileobj(response, stream, 1024 * 1024)
                body = b''
            return response.status, dict(response.getheaders()), body
        finally:
            conn.close()

    def release(self, version, base):
        status, _, raw = self.get('/v1/client-release')
        if status != 200:
            raise ValueError('release endpoint unavailable')
        value = json.loads(raw)
        source = base / 'dists' / version
        release = json.loads((source / 'release.json').read_text())
        manifest = json.loads((source / 'manifest.json').read_text())['files']
        apk = release['android']
        expected_android = {'file': apk['apk'], **manifest[apk['apk']], **{k: apk[k] for k in ('package', 'version_code', 'signer_sha256')}}
        desktop = [{'path': name, **(digest(source / name) if name in ('manifest.json', 'manifest.json.minisig') else manifest[name])}
                   for name in sorted(release['desktop']['files'] + ['manifest.json', 'manifest.json.minisig'])]
        if value != {'version': version, 'android': expected_android,
                     'desktop': {'archive': release['desktop']['archive'], 'files': desktop}}:
            raise ValueError('advertised release differs from signed dist')
        return value


def probe(base, work, port, version):
    device = Device(work, 'probe', port)
    release = device.release(version, base)
    # No member token on update paths: this is the actual 426 recovery authentication model.
    status, _, raw = device.get('/v1/client-release', version='0.0.1')
    if status != 200 or json.loads(raw) != release:
        raise ValueError('old client cannot discover update')
    status, _, _ = device.get('/v1/me', version='0.0.1', session=True)
    if status != 426:
        raise ValueError('ordinary route bypassed minimum version')
    apk = release['android']
    output = work / ('probe-' + version + '-' + str(time.monotonic_ns()) + '.apk')
    status, headers, _ = device.get('/v1/client-release/files/' + apk['file'], version='0.0.1', output=output)
    if status != 200 or headers.get('etag', headers.get('ETag')) != '"' + apk['sha256'] + '"':
        raise ValueError('old client cannot download APK with expected ETag')
    check_file(output, {k: apk[k] for k in ('size', 'sha256')})
    output.unlink()


def desktop_stage(base, work, port):
    device = Device(work, 'desktop', port)
    release = device.release('9.9.9', base)
    stage = work / 'desktop-staging'
    stage.mkdir(mode=0o700)
    (work / 'desktop-release.json').write_text(json.dumps(release))
    for file in release['desktop']['files']:
        path = member_path(stage, file['path'])
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        status, _, _ = device.get('/v1/client-release/files/' + file['path'], output=path)
        if status != 200:
            raise ValueError('desktop subset download failed')
        check_file(path, {k: file[k] for k in ('size', 'sha256')})
        path.chmod(0o600)


def assert_clean_upgrade(stack, incoming=True):
    if list((stack / 'rollback').glob('*.dump')) or list((stack / 'backups/rollback').glob('*.dump')):
        raise ValueError('plain rollback dump remains')
    # Current installer retains an empty incoming parent, but never a successful download tree.
    if incoming and (stack / 'incoming').exists() and list((stack / 'incoming').iterdir()):
        raise ValueError('successful incoming release remains')


def confirm_button(xml):
    try:
        nodes = ET.fromstring(xml).iter('node')
        buttons = []
        for node in nodes:
            a = node.attrib
            if (a.get('package') in ('com.android.packageinstaller', 'com.google.android.packageinstaller') and
                    a.get('resource-id') in ('android:id/button1', 'com.android.packageinstaller:id/ok_button') and
                    a.get('text', '').strip().lower() in ('install', 'update') and
                    a.get('enabled') == 'true' and a.get('clickable') == 'true'):
                bounds = re.fullmatch(r'\[(\d+),(\d+)\]\[(\d+),(\d+)\]', a.get('bounds', ''))
                if bounds:
                    x1, y1, x2, y2 = map(int, bounds.groups())
                    if x2 > x1 and y2 > y1:
                        buttons.append(((x1 + x2) // 2, (y1 + y2) // 2))
        return buttons[0] if len(buttons) == 1 else None
    except ET.ParseError:
        return None


def adb(serial, *args, data=None, timeout=30):
    if serial != 'emulator-5582':
        raise ValueError('update gate emulator required')
    return subprocess.run(['adb', '-s', serial, *args], input=data, capture_output=True, timeout=timeout, check=True).stdout


def android_input(work, serial):
    enrollment = json.loads((work / 'android-enroll.json').read_text())
    uri = urllib.parse.urlsplit(enrollment['uri'])
    query = dict(urllib.parse.parse_qsl(uri.query))
    if not query.get('host', '').startswith('localhost:'):
        raise ValueError('scratch enrollment host required')
    # adb reverse lets the device use the same localhost SAN and endpoints as the host probes.
    value = {'uri': enrollment['uri'], 'password': (work / 'admin.pass').read_text().strip(), 'version': '9.9.9'}
    adb(serial, 'shell', 'run-as', APP, 'mkdir', '-p', 'files')
    adb(serial, 'shell', '-T', 'run-as app.porchlight sh -c "umask 077; cat > files/update-gate.json"',
        data=json.dumps(value).encode())


def android_confirm(work, serial):
    remote = '/data/local/tmp/update-gate-window.xml'
    deadline = time.monotonic() + 45
    clicked = False
    try:
        while time.monotonic() < deadline:
            adb(serial, 'shell', 'uiautomator', 'dump', remote)
            xml = adb(serial, 'exec-out', 'cat', remote).decode()
            (work / 'android-confirm.xml').write_text(xml)
            button = confirm_button(xml)
            if button:
                adb(serial, 'shell', 'input', 'tap', *map(str, button))
                clicked = True
                break
            time.sleep(1)
    finally:
        adb(serial, 'shell', 'rm', '-f', remote)
    if not clicked:
        raise ValueError('system install confirmation not found')
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        dump = adb(serial, 'shell', 'dumpsys', 'package', APP).decode()
        (work / 'android-package.log').write_text(dump)
        if re.search(r'^\s*versionName=9.9.9\s*$', dump, re.M):
            return
        time.sleep(2)
    raise ValueError('Android did not install versionName=9.9.9')


def main():
    os.umask(0o077)
    command, *args = sys.argv[1:]
    base = ROOT / 'var/update-gate'
    if command == 'bump':
        bump(ROOT / 'var/wt/updgate-build', args[0])
    elif command == 'record-build':
        record_build(base, Path(args[0]), Path(args[1]))
    elif command == 'verify-build':
        verify_build(base)
    elif command == 'ports':
        sockets = [socket.socket() for _ in range(4)]
        try:
            for sock in sockets:
                sock.bind(('127.0.0.1', 0))
            print(' '.join(str(sock.getsockname()[1]) for sock in sockets))
        finally:
            for sock in sockets:
                sock.close()
    else:
        work = Path(args[0]).resolve()
        if not work.is_relative_to(base.resolve()) or not work.name.startswith('run-'):
            raise ValueError('scratch run directory required')
        if command == 'mirror':
            mirror(base, work / 'mirror')
        elif command == 'latest':
            latest(work / 'mirror', args[1])
        elif command == 'probe':
            probe(base, work, args[1], args[2])
        elif command == 'desktop-stage':
            desktop_stage(base, work, args[1])
        elif command == 'assert-clean-upgrade':
            assert_clean_upgrade(work / 'stack', incoming=len(args) == 1)
        elif command == 'android-input':
            android_input(work, args[1])
        elif command == 'android-confirm':
            android_confirm(work, args[1])
        else:
            raise ValueError('unknown gate command')


if __name__ == '__main__':
    main()  # Callers redirect all diagnostics into a private run log.
