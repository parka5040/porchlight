#!/usr/bin/env python3
"""Authoritative DB content identity, independent of a bundle's own inventory."""
import hashlib
import json
import os
from pathlib import Path
import re
import stat


def metadata(rows, refs):
    indexes = {(row['household_id'], row['sha256']) for row in rows}
    for ref in refs:
        for key in ('blob_sha256', 'thumbnail_sha256'):
            if ref.get(key) is not None and (ref['household_id'], ref[key]) not in indexes:
                raise ValueError('snapshot reference has no pinned blob metadata')
    result = {}
    for row in rows:
        sha, size = row['sha256'], row['size']
        if not re.fullmatch('[0-9a-f]{64}', sha) or type(size) is not int or not 0 <= size <= 26214400:
            raise ValueError('invalid blob metadata')
        name = sha[:2] + '/' + sha[2:4] + '/' + sha
        value = {'sha256': sha, 'size': size}
        if name in result and result[name] != value:
            raise ValueError('inconsistent content-addressed size')
        result[name] = value
    return dict(sorted(result.items()))


def check_digest(actual, expected):
    if actual != expected:
        raise ValueError('blob content differs from database identity')


def verify_restored(query, root):
    # Live main + thumbnail references in the restored DB, not manifest-supplied paths.
    rows = json.loads(query("SELECT COALESCE(json_agg(to_jsonb(b)), '[]') FROM porchlight.blobs b WHERE EXISTS(SELECT 1 FROM porchlight.attachments a WHERE a.household_id=b.household_id AND a.deleted_at IS NULL AND (a.blob_sha256=b.sha256 OR a.thumbnail_sha256=b.sha256));"))
    refs = json.loads(query("SELECT COALESCE(json_agg(to_jsonb(a)), '[]') FROM porchlight.attachments a WHERE a.deleted_at IS NULL;"))
    for name, expected in metadata(rows, refs).items():
        path = Path(root) / name
        if any(p.is_symlink() for p in (path, *path.parents)):
            raise ValueError('symlink restored blob')
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, 'rb') as stream:
            if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                raise ValueError('non-regular restored blob')
            digest, size = hashlib.sha256(), 0
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(chunk)
                size += len(chunk)
            check_digest({'sha256': digest.hexdigest(), 'size': size}, expected)
