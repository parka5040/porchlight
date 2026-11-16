#!/usr/bin/env python3
"""Enroll N synthetic members' devices through porch's production TLS protocol."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import urllib.parse

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import private_file, runtime_dir, write_private


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for key in ['socket', 'token-file', 'seed-dir', 'out', 'base-url', 'target-label']:
        p.add_argument('--'+key, required=True)
    p.add_argument('--porch', default='porch')
    p.add_argument('--count', type=int, default=6)
    p.add_argument('--enroll-addr')
    a = p.parse_args()
    if not 1 <= a.count <= 256: raise ValueError('device count cap is 256')
    endpoint = urllib.parse.urlsplit(a.base_url)
    if endpoint.scheme != 'https' or not endpoint.hostname or endpoint.username or endpoint.password or endpoint.query or endpoint.fragment or endpoint.path not in ('', '/'):
        raise ValueError('root HTTPS base URL required')
    out, seed = runtime_dir(a.out), runtime_dir(a.seed_dir)
    token = private_file(a.token_file)
    manifest = json.loads(private_file(seed / 'manifest.json').read_text())
    out.mkdir(parents=True, exist_ok=True, mode=0o700)
    devices = []
    for i in range(a.count):
        label = f'device-{i:04d}'
        device = out / label
        user = manifest['prefix'] + f'-member-{i % 6}'
        cfg = {'base_url': a.base_url.rstrip('/'), 'target_label': a.target_label}
        if device.exists() and (device / 'target.json').exists():
            if json.loads(private_file(device / 'target.json').read_text()) != cfg:
                raise ValueError('existing device target mismatch')
        if not (device / 'session.token').exists():
            env = dict(os.environ)
            env['PORCHLIGHT_PASSWORD'] = private_file(seed / 'passwords' / (user+'.password')).read_text()
            cmd = [a.porch, '--socket', a.socket, '--token-file', str(token), '--json', 'dev-init',
                   '--out', str(out), '--user', user, '--devices', label+':desktop']
            if a.enroll_addr: cmd += ['--enroll-addr', a.enroll_addr]
            r = subprocess.run(cmd, env=env, capture_output=True, timeout=120)
            if r.returncode: raise ValueError('enrollment failed; pending credentials retained; retry same command')
        private_file(device / 'session.token')
        private_file(device / 'client.key')
        write_private(device / 'target.json', json.dumps(cfg))
        devices.append(str(device))
    write_private(out / 'devices.json', json.dumps({'devices': devices, 'target_label': a.target_label}, indent=2))
    print(f'enrolled/retained {len(devices)} synthetic devices (credentials not printed)')

if __name__ == '__main__':
    try: main()
    except Exception:
        print('FAIL: device provisioning refused or failed; credentials not printed', file=sys.stderr)
        sys.exit(1)
