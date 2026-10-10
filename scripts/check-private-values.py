#!/usr/bin/env python3
"""Fail when a committed file contains a value from the gitignored machine profile (.env, see .env.example).

Checked: PORCHLIGHT_DEPLOY_HOST, PORCHLIGHT_DEPLOY_DATA_MOUNT, PORCHLIGHT_DEPLOY_LAN (the address, and for a /24 or
longer prefix every address on that network), PORCHLIGHT_RELEASE_REPO (owner only), PORCHLIGHT_PRIVATE_STRINGS and
the home directory. Without .env (a fresh clone) only the home directory is checked. Findings name the source key,
never the value.

Usage: check-private-values.py [--root DIR] [--home DIR]
"""
import argparse
import ipaddress
import pathlib
import re
import sys

# A superset of what git tracks (.gitignore): scanning an ignored file is harmless, skipping a tracked one is not.
SKIP_DIR_NAMES = {'.git', 'target', 'build', '.gradle', '.kotlin', 'node_modules', '__pycache__', '.claude', '.idea'}
SKIP_ROOT_PATHS = {'var', 'dist', 'docs/plans', 'docs/operations/gates'}
WORD = 'A-Za-z0-9_'


def profile(root):
    values = {}
    path = root / '.env'
    if path.is_file():
        for line in path.read_text().splitlines():
            key, sep, value = line.partition('=')
            if sep and key.startswith('PORCHLIGHT_') and value.strip():
                values[key] = value.strip()
    return values


def literal(text):
    """Case-insensitive match of text, not glued to neighbouring word characters."""
    left = f'(?<![{WORD}])' if text[0].isalnum() or text[0] == '_' else ''
    right = f'(?![{WORD}])' if text[-1].isalnum() or text[-1] == '_' else ''
    return left + re.escape(text) + right


def needles(values, home):
    found = []  # (source key, compiled pattern)
    add = lambda key, pattern: found.append((key, re.compile(pattern.encode('latin-1'), re.I)))
    host = values.get('PORCHLIGHT_DEPLOY_HOST', '')
    for part in filter(None, host.split('@')):
        if len(part) >= 3:
            add('PORCHLIGHT_DEPLOY_HOST', literal(part))
    mount = values.get('PORCHLIGHT_DEPLOY_DATA_MOUNT', '').rstrip('/')
    if mount.count('/') >= 2:  # a bare top-level directory such as /srv is too generic to flag
        add('PORCHLIGHT_DEPLOY_DATA_MOUNT', re.escape(mount) + f'(?![{WORD}.-])')
    lan = values.get('PORCHLIGHT_DEPLOY_LAN', '')
    if lan:
        interface = ipaddress.IPv4Interface(lan)
        add('PORCHLIGHT_DEPLOY_LAN', r'(?<![\d.])' + re.escape(str(interface.ip)) + r'(?![\d])')
        if interface.network.prefixlen >= 24:
            prefix = '.'.join(str(interface.network.network_address).split('.')[:3]) + '.'
            add('PORCHLIGHT_DEPLOY_LAN', r'(?<![\d.])' + re.escape(prefix) + r'\d')
    owner = values.get('PORCHLIGHT_RELEASE_REPO', '').partition('/')[0]
    if len(owner) >= 3:
        add('PORCHLIGHT_RELEASE_REPO', f'(?<![{WORD}])' + re.escape(owner) + f'(?![{WORD}])')
    for text in values.get('PORCHLIGHT_PRIVATE_STRINGS', '').split(','):
        if len(text.strip()) >= 3:
            add('PORCHLIGHT_PRIVATE_STRINGS', literal(text.strip()))
    home = str(home).rstrip('/')
    if home.count('/') >= 2:
        add('home directory', re.escape(home) + f'(?![{WORD}.-])')
    return found


def files(root):
    for path in sorted(root.rglob('*')):
        relative = path.relative_to(root)
        parts = relative.parts
        if any(part in SKIP_DIR_NAMES for part in parts[:-1]) or path.name in SKIP_DIR_NAMES:
            continue
        if any(relative.as_posix() == skip or relative.as_posix().startswith(skip + '/') for skip in SKIP_ROOT_PATHS):
            continue
        if path.name == 'CLAUDE.md' or (path.name.startswith('.env') and not path.name.endswith('.example')):
            continue
        if path.is_file() and not path.is_symlink():
            yield relative, path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[1])
    parser.add_argument('--home', type=pathlib.Path, default=pathlib.Path.home())
    args = parser.parse_args()
    root = args.root.resolve()
    checks = needles(profile(root), args.home)
    failures = scanned = 0
    for relative, path in files(root):
        scanned += 1
        data = path.read_bytes()
        for key, pattern in checks:
            for match in pattern.finditer(data):
                line = data.count(b'\n', 0, match.start()) + 1
                print(f'FAIL private value ({key}) in {relative}:{line}')
                failures += 1
    if failures:
        print(f'{failures} private value(s) in committed files; move them to .env (see .env.example)')
        return 1
    print(f'PASS no private values in {scanned} committed files ({len(checks)} checks'
          + ('' if (root / '.env').is_file() else '; no .env, home directory only') + ')')
    return 0


if __name__ == '__main__':
    sys.exit(main())
