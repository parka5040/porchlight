#!/usr/bin/env python3
"""Verify O2 household export v2 without extracting or applying it. No secret diagnostics."""
import gzip
import hashlib
import json
import pathlib
import re
import sys
import tarfile

def verify(path):
    seen = set(); rows = {}; blobs = []; manifest = None; completion = None; last = None
    with gzip.open(path, 'rb') as stream:
        with tarfile.open(fileobj=stream, mode='r|') as archive:
            for entry in archive:
                name = entry.name
                if completion is not None or not entry.isfile() or name in seen:
                    raise ValueError('duplicate, non-file or post-completion entry')
                if name.startswith('/') or '..' in pathlib.PurePosixPath(name).parts:
                    raise ValueError('unsafe path')
                seen.add(name); last = name
                body = archive.extractfile(entry)
                if name in ('data.json', 'complete.json'):
                    if entry.size > 8 * 1024 * 1024: raise ValueError('metadata too large')
                    obj = json.load(body)
                    if name == 'data.json': manifest = obj
                    else: completion = obj
                elif re.fullmatch(r'tables/[a-z_][a-z0-9_]*\.jsonl', name):
                    count = 0
                    for line in body:
                        if not isinstance(json.loads(line), dict): raise ValueError('invalid row')
                        count += 1
                    rows[name[7:-6]] = count
                elif re.fullmatch(r'blobs/[0-9a-f]{2}/[0-9a-f]{2}/[0-9a-f]{64}', name):
                    digest = hashlib.sha256()
                    while chunk := body.read(65536): digest.update(chunk)
                    expected = name.rsplit('/', 1)[1]
                    if name != f'blobs/{expected[:2]}/{expected[2:4]}/{expected}' or digest.hexdigest() != expected:
                        raise ValueError('blob hash mismatch')
                    blobs.append(expected)
                else: raise ValueError('unexpected entry')
            # Include tarfile's read-ahead buffer: bytes after the end marker must be zero padding.
            while chunk := archive.fileobj.read(65536):
                if any(chunk): raise ValueError('nonzero bytes after tar end')
        # Also drain the gzip reader to enforce CRC, length and trailer.
        while chunk := stream.read(65536):
            if any(chunk): raise ValueError('nonzero bytes after tar end')
    if not manifest or manifest.get('format_version') != 2 or last != 'complete.json' or not completion:
        raise ValueError('incomplete export')
    if completion.get('format_version') != 2 or completion.get('complete') is not True:
        raise ValueError('invalid completion marker')
    if manifest.get('tables') != {name: f'tables/{name}.jsonl' for name in rows}:
        raise ValueError('manifest inventory mismatch')
    if completion.get('table_rows') != rows or completion.get('blob_count') != len(blobs) or sorted(completion.get('blob_sha256', [])) != sorted(blobs):
        raise ValueError('completion inventory mismatch')
    return rows

if __name__ == '__main__':
    try:
        verify(pathlib.Path(sys.argv[1])); print('PASS export complete.json inventory and gzip trailer')
    except Exception:
        print('FAIL incomplete or invalid household export'); sys.exit(1)
