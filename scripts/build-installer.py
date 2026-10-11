#!/usr/bin/env python3
"""Build the single-file trust bootstrap; no code from a release runs before verification."""
from pathlib import Path
import hashlib
import sys


def build(root, public):
    template = (root / 'deploy/install.sh').read_text()
    helpers = (root / 'deploy/lib/operations.sh').read_text()
    helpers = helpers[helpers.index('version_code()'):]
    python = (root / 'deploy/lib/operations.py').read_text()
    # Match core/build.rs's policy input without embedding plaintext password fixtures.
    entries = (line.strip().lower() for line in (root / 'server/core/data/breached-passwords.txt').read_text().splitlines())
    hashes = sorted({hashlib.sha256(line.encode()).hexdigest() for line in entries if len(line) >= 12 and not line.startswith('#')})
    python = python.replace('@BREACHED_PASSWORD_HASHES@', ' '.join(hashes))
    python = python.replace('@STORAGE_ZONES@', ' '.join((root / 'deploy/lib/storage-zones.txt').read_text().split()))
    inline = helpers + "\nops_python() {\n  python3 -I -B - \"$@\" <<'PORCHLIGHT_TRUSTED_PYTHON'\n" + python + '\nPORCHLIGHT_TRUSTED_PYTHON\n}\n'
    assert template.count('# @INLINE_OPERATIONS@') == 1
    return template.replace('@MINISIGN_PUBLIC_KEY@', public).replace('# @INLINE_OPERATIONS@', inline)


if __name__ == '__main__':
    root, destination, public = sys.argv[1:]
    Path(destination).write_text(build(Path(root), public))
