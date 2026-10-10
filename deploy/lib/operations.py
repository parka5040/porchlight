#!/usr/bin/env python3
"""Offline deployment helpers: standard library only, never log secret input."""
import datetime as dt
import hashlib
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


def extract(archive, destination):
    # Validate the entire archive before writing anything, including links and duplicate members.
    with tarfile.open(archive) as t:
        seen = set()
        for m in t.getmembers():
            name = m.name.rstrip('/')
            if name == '.':
                if m.isdir():
                    continue
                raise ValueError('unsafe archive root')
            if name.startswith('./'):
                name = name[2:]
            member_path(name)
            if name in seen or not (m.isfile() or m.isdir()):
                raise ValueError('unsafe or duplicate archive member')
            seen.add(name)
        destination.mkdir(mode=0o700, parents=True, exist_ok=False)
        t.extractall(destination, filter='data')


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
    elif cmd == 'extract':
        extract(Path(args[0]), Path(args[1]))
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
