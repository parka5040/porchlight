#!/usr/bin/env python3
"""Stage a cold desktop copy where the laptop's age Docker wrapper can see it. Never decrypt here."""
import hashlib
import json
import os
import pathlib
import re
import shutil
import stat
import sys
import tempfile
ROOT = pathlib.Path(__file__).resolve().parents[2]

def opened(path):
    if path.is_symlink(): raise ValueError('unsafe source')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    if not stat.S_ISREG(os.fstat(fd).st_mode):
        os.close(fd)
        raise ValueError('not a regular file')
    return os.fdopen(fd, 'rb')

def stage(bundle, receipt, destination):
    if destination.is_symlink() or destination.stat().st_mode & 0o077: raise ValueError('destination must be private')
    with opened(receipt) as source:
        raw = source.read(8 * 1024 * 1024 + 1)
        if len(raw) > 8 * 1024 * 1024: raise ValueError('receipt too large')
        expected = json.loads(raw)
    if not isinstance(expected.get('size'), int) or expected['size'] <= 0 or not re.fullmatch('[0-9a-f]{64}', expected.get('sha256', '')):
        raise ValueError('invalid receipt')
    target = destination / 'bundle.tar.age'
    with opened(bundle) as source, os.fdopen(os.open(target, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), 'wb') as output:
        shutil.copyfileobj(source, output, 65536)
        output.flush(); os.fsync(output.fileno())
    # Reopen the copied bytes, rather than merely hashing the source during copy.
    digest = hashlib.sha256()
    with opened(target) as copied:
        while chunk := copied.read(65536): digest.update(chunk)
    if target.stat().st_size != expected['size'] or digest.hexdigest() != expected['sha256']:
        raise ValueError('copied bundle receipt mismatch')
    with os.fdopen(os.open(destination / 'receipt.json', os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), 'wb') as output:
        output.write(raw); output.flush(); os.fsync(output.fileno())
    return target

if __name__ == '__main__':
    try:
        if len(sys.argv) != 3: raise ValueError('bundle and receipt required')
        os.umask(0o077)
        destination = pathlib.Path(tempfile.mkdtemp(prefix='pulled-restore.', dir=ROOT / 'var'))
        print(stage(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), destination))
    except Exception:
        print('FAIL pulled bundle staging or receipt verification; inspect private var staging', file=sys.stderr)
        sys.exit(1)
