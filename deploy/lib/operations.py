#!/usr/bin/env python3
"""Offline deployment helpers: standard library only, never log secret input."""
import datetime as dt
import hashlib
import ipaddress
import gzip
import shutil
import tempfile
import json
import os
from pathlib import Path, PurePosixPath
import re
import sys
import tarfile
import urllib.parse
import urllib.request
from zoneinfo import ZoneInfo


def version_code(version):
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', version):
        raise ValueError('VERSION must be MAJOR.MINOR.PATCH')
    major, minor, patch = map(int, version.split('.'))
    code = major * 10000 + minor * 100 + patch
    if minor > 99 or patch > 99 or not 0 < code <= 2100000000:
        raise ValueError('VERSION does not fit Android versionCode')
    return code


def source_kind(source):
    if not source or source.startswith('-') or '\n' in source or '\r' in source:
        raise ValueError('invalid source')
    if ':' not in source:
        return 'local'
    if re.fullmatch(r'[a-zA-Z0-9_][a-zA-Z0-9_.-]*@[a-zA-Z0-9][a-zA-Z0-9_.-]*:/[^\r\n]*', source):
        return 'ssh'
    raise ValueError('source must be a directory or user@host:/absolute/path')


def member_path(name):
    p = PurePosixPath(name)
    if not name or p.is_absolute() or any(x in ('..', '') for x in p.parts) or '\\' in name or str(p) != name:
        raise ValueError('unsafe member path')
    return p


def digest(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''):
            h.update(chunk)
    return {'sha256': h.hexdigest(), 'size': path.stat().st_size}


def members(root):
    files = {}
    for p in sorted(root.rglob('*')):
        name = p.relative_to(root).as_posix()
        if p.is_symlink():
            raise ValueError('symlinks forbidden in bundle')
        if not p.is_file() and not p.is_dir():
            raise ValueError('non-regular bundle member')
        if p.is_file() and name not in ('manifest.json', 'manifest.json.minisig'):
            member_path(name)
            files[name] = digest(p)
    return files


def now():
    return dt.datetime.now(dt.timezone.utc).isoformat().replace('+00:00', 'Z')


def manifest(root, version, metadata=None):
    version_code(version)
    value = {'format_version': 1, 'version': version, 'created_at': now(), 'files': members(root)}
    if metadata:
        value.update(metadata)
    (root / 'manifest.json').write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')
    return value


def verify(root, subset=False):
    if (root / 'manifest.json').is_symlink():
        raise ValueError('symlink manifest')
    m = json.loads((root / 'manifest.json').read_text())
    if m.get('format_version') != 1 or not isinstance(m.get('files'), dict) or not m['files']:
        raise ValueError('invalid manifest')
    version_code(m['version'])
    present = members(root)
    for name, info in m['files'].items():
        member_path(name)
        if name in ('manifest.json', 'manifest.json.minisig'):
            raise ValueError('reserved manifest member')
        if not subset or name in present:
            if present.get(name) != info:
                raise ValueError('unsafe, missing or mismatched member')
    if present.keys() - m['files'].keys():
        raise ValueError('unlisted member')
    if subset:
        if 'release.json' not in present:
            raise ValueError('missing release descriptor')
        release = json.loads((root / 'release.json').read_text())
        if release.get('format_version') != 1 or release.get('version') != m['version']:
            raise ValueError('invalid release descriptor')
        required = release['desktop']['files']
        if not isinstance(required, list) or not required or 'release.json' not in required:
            raise ValueError('invalid desktop inventory')
        for name in required:
            member_path(name)
            if name not in present:
                raise ValueError('missing desktop member')
    return m


def release_repo(value):
    if not re.fullmatch(r'[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})/[A-Za-z0-9._-]{1,100}', value):
        raise ValueError('invalid release repository')
    return value


def fetch_release(repo, override, testing, release, asset, destination):
    release_repo(repo)
    if release != 'latest':
        version_code(release)
    if asset not in ('manifest.json', 'manifest.json.minisig', f'porchlight-{release}-dist.tar'):
        raise ValueError('invalid release asset')
    limit = 2 * 1024**3 if asset.endswith('-dist.tar') else 1024**2
    base = override or f'https://github.com/{repo}/releases'
    parsed = urllib.parse.urlsplit(base)
    local = (testing == '1' and base.startswith('http://127.0.0.1:')
             and parsed.hostname == '127.0.0.1' and parsed.port is not None)
    if (not base.startswith('https://') and not local) or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError('HTTPS release base required')
    origin = (parsed.scheme, parsed.netloc)

    class Redirects(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl):
            url = urllib.parse.urlsplit(newurl)
            allowed = url.scheme == 'https' and (url.hostname == 'github.com'
                       or (url.hostname or '').endswith('.githubusercontent.com'))
            if local and (url.scheme, url.netloc) == origin:
                allowed = True
            if not allowed or url.username or url.password:
                raise ValueError('release redirect rejected')
            return super().redirect_request(req, fp, code, msg, headers, newurl)

    path = 'latest/download' if release == 'latest' else f'download/v{release}'
    request = urllib.request.Request(f'{base.rstrip("/")}/{path}/{asset}', headers={'Accept-Encoding': 'identity'})
    opener = urllib.request.build_opener(Redirects())
    # A socket timeout bounds both connection establishment and each blocking read.
    with opener.open(request, timeout=30) as response:
        length = response.headers.get('Content-Length')
        if length is not None and (int(length) < 0 or int(length) > limit):
            raise ValueError('release response too large')
        fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, 'wb') as output:
            size = 0
            while chunk := response.read(min(1024**2, limit - size + 1)):
                size += len(chunk)
                if size > limit:
                    raise ValueError('release response too large')
                output.write(chunk)
            if length is not None and size != int(length):
                raise ValueError('incomplete release response')


def valid_dns(value):
    return (len(value) <= 253 and not re.fullmatch(r'[0-9.]+', value)
            and all(re.fullmatch(r'[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?', label)
                    for label in value.split('.')))


def validate_restore_target(env_path, *pairs):
    values = dict(line.split('=', 1) for line in Path(env_path).read_text().splitlines() if '=' in line)
    overrides = dict(zip(pairs[::2], pairs[1::2], strict=True))
    for key, value in overrides.items():
        if not value or any(c in value for c in '\r\n$#\"\''):
            raise ValueError('invalid restore target setting')
        if key in ('PG_PORT', 'LAN_PORT', 'ENROLL_PORT'):
            if not re.fullmatch(r'[0-9]{1,5}', value) or not 1 <= int(value) <= 65535:
                raise ValueError('invalid restore port')
        elif key == 'LAN_BIND':
            ipaddress.ip_address(value)
        elif key in ('BACKEND_SUBNET', 'INGRESS_SUBNET'):
            ipaddress.ip_network(value)
        elif key == 'ENROLL_CIDRS':
            for cidr in value.split(','):
                ipaddress.ip_network(cidr.strip(), strict=False)
        elif key in ('ENROLL_HOST', 'PORCHLIGHT_ENDPOINTS'):
            for endpoint in value.split(','):
                url = urllib.parse.urlsplit(('https://' if key == 'ENROLL_HOST' else '') + endpoint)
                if (url.scheme != 'https' or not url.hostname or url.username or url.password
                        or url.path not in ('', '/') or url.query or url.fragment or url.port == 0):
                    raise ValueError('invalid restore endpoint')
                try:
                    ipaddress.ip_address(url.hostname)
                except ValueError:
                    if not valid_dns(url.hostname):
                        raise ValueError('invalid restore host') from None
        else:
            raise ValueError('unknown restore target setting')
    values.update(overrides)
    backend = ipaddress.ip_network(values.get('BACKEND_SUBNET', '172.29.250.0/24'))
    ingress = ipaddress.ip_network(values.get('INGRESS_SUBNET', '172.29.251.0/24'))
    if backend.overlaps(ingress):
        raise ValueError('overlapping restore networks')


def validate_init(password_file, username, display, household, zone, recipient, *sans):
    with open(password_file) as password_input:
        password = password_input.readline(8193).removesuffix('\n').removesuffix('\r')
    breached = set('@BREACHED_PASSWORD_HASHES@'.split())
    if not 12 <= len(password) <= 1024 or hashlib.sha256(password.lower().encode()).hexdigest() in breached:
        raise ValueError('admin password rejected')
    if not re.fullmatch(r'[a-z0-9][a-z0-9._-]{1,31}', username) or not 1 <= len(display.strip()) <= 60 or not 1 <= len(household.strip()) <= 100:
        raise ValueError('invalid initial admin or household')
    if not re.fullmatch(r'age1[023456789acdefghjklmnpqrstuvwxyz]{58}', recipient):
        raise ValueError('invalid age recipient')
    if zone not in '@STORAGE_ZONES@'.split():
        raise ValueError('invalid storage timezone')
    for kind, value in zip(sans[::2], sans[1::2], strict=True):
        if kind == '--san-ip':
            ipaddress.ip_address(value)
        elif kind != '--san-dns' or not valid_dns(value):
            raise ValueError('invalid DNS SAN')
    for cidr in os.environ.get('ENROLL_CIDRS', '').split(','):
        ipaddress.ip_network(cidr.strip(), strict=False)
    for key in ('PG_PORT', 'LAN_PORT', 'ENROLL_PORT'):
        if not 1 <= int(os.environ.get(key, '8443')) <= 65535:
            raise ValueError('invalid port')


def atomic_text(path, text):
    fd, name = tempfile.mkstemp(prefix='.' + path.name + '.', dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as output:
            output.write(text); output.flush(); os.fsync(output.fileno())
        os.replace(name, path)
        fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def extract(archive, destination, inventory=None):
    # Bound raw headers too: TarFile.getmembers/next can allocate hidden PAX/sparse payloads.
    expected = None
    directories = None
    count_limit, byte_limit = (100000, 4 * 1024**3) if inventory == '--desktop' else (1000000, 1024**4)
    if inventory is not None and inventory != '--desktop':
        manifest_path = Path(inventory)
        data = json.loads(manifest_path.read_text())
        if data.get('format_version') != 1 or not isinstance(data.get('files'), dict) or not data['files']:
            raise ValueError('invalid manifest')
        expected = {}
        directories = {'.'}
        for name, info in data['files'].items():
            path = member_path(name)
            if name in ('manifest.json', 'manifest.json.minisig') or type(info['size']) is not int or info['size'] < 0:
                raise ValueError('invalid inventory')
            expected[name] = info['size']
            directories.update(str(p) for p in path.parents)
        for name in ('manifest.json', 'manifest.json.minisig'):
            expected[name] = manifest_path.with_name(name).stat().st_size
        count_limit = len(expected) + len(directories)
        byte_limit = sum(expected.values())

    def scan(write=False):
        seen = set(); total = 0; headers = 0; extended = {}; extensions = 0
        with open(archive, 'rb') as raw:
            magic = raw.read(2); raw.seek(0)
            stream = gzip.GzipFile(fileobj=raw) if magic == b'\x1f\x8b' else raw
            try:
                while True:
                    block = stream.read(512)
                    if block == bytes(512):
                        if extended or stream.read(512) != bytes(512):
                            raise ValueError('invalid tar terminator')
                        padding = stream.read(65537)
                        if len(padding) > 65536 or any(padding):
                            raise ValueError('invalid tar padding')
                        break
                    if len(block) != 512:
                        raise ValueError('truncated archive')
                    headers += 1
                    if headers > 3 * count_limit:
                        raise ValueError('too many archive headers')
                    entry = tarfile.TarInfo.frombuf(block, 'utf-8', 'strict')
                    if entry.type in (tarfile.XHDTYPE, tarfile.GNUTYPE_LONGNAME):
                        extensions += 1
                        if extensions > 2 or not 0 < entry.size <= 16384:
                            raise ValueError('oversize or repeated extended header')
                        body = stream.read((entry.size + 511) // 512 * 512)
                        if len(body) != (entry.size + 511) // 512 * 512:
                            raise ValueError('truncated extended header')
                        body = body[:entry.size]
                        if entry.type == tarfile.GNUTYPE_LONGNAME:
                            extended['path'] = body.rstrip(b'\0').decode('utf-8')
                        else:
                            while body:
                                length, sep, _ = body.partition(b' ')
                                if not sep or not length.isdigit() or not len(length) + 3 <= int(length) <= len(body):
                                    raise ValueError('invalid PAX record')
                                record, body = body[len(length)+1:int(length)], body[int(length):]
                                key, sep, value = record[:-1].partition(b'=')
                                if not sep or not record.endswith(b'\n') or key not in (b'path', b'size', b'mtime', b'atime', b'ctime', b'uid', b'gid', b'uname', b'gname'):
                                    raise ValueError('unsupported PAX record')
                                extended[key.decode()] = value.decode('utf-8')
                        continue
                    name = extended.get('path', entry.name).rstrip('/')
                    size = int(extended.get('size', entry.size))
                    extended = {}; extensions = 0
                    if name.startswith('./'): name = name[2:]
                    member_path(name)
                    directory = entry.type == tarfile.DIRTYPE
                    if entry.type not in (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE) or name in seen or size < 0 or (directory and size):
                        raise ValueError('unsafe or duplicate archive member')
                    seen.add(name); total += size
                    if len(seen) > count_limit or total > byte_limit:
                        raise ValueError('archive expansion budget exceeded')
                    if expected is not None:
                        if (directory and name not in directories) or (not directory and expected.get(name) != size):
                            raise ValueError('unlisted member or size/type mismatch')
                    target = destination / name
                    output = None
                    if write:
                        if directory:
                            target.mkdir(mode=0o700, parents=True, exist_ok=True)
                        else:
                            target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                            output = open(target, 'xb')
                    try:
                        remaining = size
                        while remaining:
                            chunk = stream.read(min(1024**2, remaining))
                            if not chunk: raise ValueError('truncated member')
                            if output: output.write(chunk)
                            remaining -= len(chunk)
                        padding = (-size) % 512
                        if len(stream.read(padding)) != padding: raise ValueError('truncated member padding')
                    finally:
                        if output:
                            output.close()
                            target.chmod(entry.mode & 0o777 & ~0o022)
                if expected is not None and expected.keys() - seen:
                    raise ValueError('missing inventory member')
            finally:
                if stream is not raw: stream.close()

    scan()  # No destination exists until ALL headers, sizes and names have passed.
    destination.mkdir(mode=0o700, parents=True, exist_ok=False)
    try:
        scan(write=True)
    except BaseException:
        shutil.rmtree(destination)
        raise


def retained(names):
    dates = []
    for n in names:
        match = re.fullmatch(r'porchlight-(\d{8}T\d{6}(?:\d{6})?Z)\.tar\.age', n)
        if match:
            fmt = '%Y%m%dT%H%M%S%fZ' if len(match[1]) > 16 else '%Y%m%dT%H%M%SZ'
            dates.append((dt.datetime.strptime(match[1], fmt), n))
    dates.sort(reverse=True)
    daily, weekly = {}, {}
    for date, name in dates:
        daily.setdefault(date.date(), name)
        weekly.setdefault(date.isocalendar()[:2], name)
    return sorted(set(list(daily.values())[:14]) | set(list(weekly.values())[:8]))


def next_due(zone, instant):
    z = ZoneInfo(zone)
    utc = dt.datetime.fromisoformat(instant.replace('Z', '+00:00'))
    local = utc.astimezone(z)
    day = local.date()
    candidate = dt.datetime.combine(day, dt.time(3), z)
    if candidate <= local:
        candidate = dt.datetime.combine(day + dt.timedelta(days=1), dt.time(3), z)
    # At 03:00 there are rare zone gaps; round-trip shifts to the next valid time.
    candidate = candidate.astimezone(dt.timezone.utc)
    return candidate.isoformat().replace('+00:00', 'Z')


def backup_due(last_success, next_due_at, instant):
    if last_success is None or next_due_at is None:
        return True
    parse = lambda s: dt.datetime.fromisoformat(s.replace('Z', '+00:00'))
    current = parse(instant)
    return current - parse(last_success) > dt.timedelta(hours=24) or parse(next_due_at) <= current


def main():
    cmd, *args = sys.argv[1:]
    if cmd == 'version-code':
        print(version_code(args[0]))
    elif cmd == 'source-kind':
        print(source_kind(args[0]))
    elif cmd == 'manifest':
        manifest(Path(args[0]), args[1])
    elif cmd == 'verify':
        verify(Path(args[0]), subset=args[1:] == ['--subset'])
    elif cmd == 'fetch-release':
        fetch_release(*args)
    elif cmd == 'release-repo':
        print(release_repo(args[0]))
    elif cmd == 'manifest-version':
        value = json.loads(Path(args[0]).read_text())['version']
        version_code(value)
        print(value)
    elif cmd == 'env-value':
        values = dict(line.split('=', 1) for line in Path(args[0]).read_text().splitlines()
                      if '=' in line and not line.startswith('#'))
        if args[1] not in values:
            sys.exit(3)
        print(values[args[1]])
    elif cmd == 'release-source':
        path = Path(args[0]) / 'release.json'
        value = json.loads(path.read_text())['source']['github'] if path.exists() else None
        if value is not None:
            print(release_repo(value))
    elif cmd == 'validate-init':
        validate_init(*args)
    elif cmd == 'validate-restore-target':
        validate_restore_target(*args)
    elif cmd == 'init-output-valid':
        output = json.loads(Path(args[0]).read_text())
        if not output.get('household_id') or not output.get('bootstrap_token'):
            raise ValueError('incomplete init delivery')
    elif cmd == 'provision-env-check':
        expected, saved = (dict(line.split('=', 1) for line in Path(path).read_text().splitlines() if '=' in line) for path in args)
        public = {key: value for key, value in expected.items() if not key.endswith('_PASSWORD')}
        if any(saved.get(key) != value for key, value in public.items()):
            raise ValueError('provisioning deployment inputs differ')
    elif cmd == 'provision-marker':
        path = Path(args[0])
        text = json.dumps(args[1:], separators=(',', ':')) + '\n'
        if path.is_symlink() or (path.exists() and path.read_text() != text):
            raise ValueError('provisioning inputs differ; retry the original release and settings')
        if not path.exists(): atomic_text(path, text)
    elif cmd == 'same-tree':
        def tree(path):
            result = members(path)
            for node in path.rglob('*'):
                name = node.relative_to(path).as_posix()
                if node.is_dir(): result[name] = {'directory': True}
                else: result[name]['execute'] = node.stat().st_mode & 0o111
            return result
        if tree(Path(args[0])) != tree(Path(args[1])):
            raise ValueError('incomplete desktop tree')
    elif cmd == 'atomic-copy':
        atomic_text(Path(args[1]), Path(args[0]).read_text())
    elif cmd == 'atomic-text':
        atomic_text(Path(args[0]), args[1] + '\n')
    elif cmd == 'extract':
        extract(Path(args[0]), Path(args[1]), args[2] if len(args) > 2 else None)
    elif cmd == 'retention-selection':
        print('\n'.join(retained(args)))
    elif cmd == 'retain':
        directory = Path(args[0])
        keep = retained([p.name for p in directory.iterdir()])
        for p in directory.glob('porchlight-*.tar.age'):
            if re.fullmatch(r'porchlight-\d{8}T\d{6}(?:\d{6})?Z\.tar\.age', p.name) and p.name not in keep:
                p.unlink()
    elif cmd == 'next-due':
        print(next_due(*args))
    else:
        raise ValueError('unknown command')


if __name__ == '__main__':
    try:
        main()
    except Exception:
        print('operations: validation or I/O failed', file=sys.stderr)
        sys.exit(1)
