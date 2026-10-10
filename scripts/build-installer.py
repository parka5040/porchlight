#!/usr/bin/env python3
"""Build the single-file trust bootstrap; no code from a release runs before verification."""
from pathlib import Path
import sys


def build(root, public):
    template = (root / 'deploy/install.sh').read_text()
    helpers = (root / 'deploy/lib/operations.sh').read_text()
    helpers = helpers[helpers.index('version_code()'):]
    python = (root / 'deploy/lib/operations.py').read_text()
    inline = helpers + "\nops_python() {\n  python3 -I -B - \"$@\" <<'PORCHLIGHT_TRUSTED_PYTHON'\n" + python + '\nPORCHLIGHT_TRUSTED_PYTHON\n}\n'
    assert template.count('# @INLINE_OPERATIONS@') == 1
    return template.replace('@MINISIGN_PUBLIC_KEY@', public).replace('# @INLINE_OPERATIONS@', inline)


if __name__ == '__main__':
    root, destination, public = sys.argv[1:]
    Path(destination).write_text(build(Path(root), public))
