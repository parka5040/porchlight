#!/usr/bin/env python3
"""Offline interpolation contract: execute env writers, never Compose or real crypto."""
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
(root / 'var/tests').mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix='compose-env.', dir=root / 'var/tests') as work:
    work = Path(work)
    (work / 'scripts').mkdir()
    (work / 'var').mkdir()
    (work / 'bin').mkdir()
    shutil.copy(root / 'VERSION', work / 'VERSION')
    for name in ('env.sh', 'dev-env.sh', 'local-env.sh'):
        shutil.copy(root / 'scripts' / name, work / 'scripts' / name)
    fake = work / 'bin/openssl'
    fake.write_text('#!/bin/sh\nprintf "fixture-password\\n"\n')
    fake.chmod(0o755)
    # No ambient dev environment: proves the same contract for a fresh caller.
    env = {'PATH': str(work / 'bin') + ':/usr/bin:/bin', 'HOME': str(work)}
    subprocess.run(['bash', str(work / 'scripts/dev-env.sh')], env=env, check=True, stdout=subprocess.DEVNULL)

    def keys(file):
        return {line.split('=', 1)[0]: line.split('=', 1)[1] for line in file.read_text().splitlines()
                if '=' in line and not line.startswith('#')}

    dev = keys(work / 'var/dev.env')
    # Test an incorrect inherited VERSION too: repository VERSION must win.
    version = subprocess.check_output(['bash', '-c', '. "$1"; printf "%s" "$PORCHLIGHT_VERSION"',
                                      'fixture', str(work / 'scripts/env.sh')],
                                     env=dict(env, PORCHLIGHT_VERSION='wrong'), text=True)
    assert version == (root / 'VERSION').read_text().strip()
    assert dev['PORCHLIGHT_VERSION'] == version
    m1 = (root / 'deploy/scripts/m1-demo.sh').read_text()
    assert m1.index('scripts/dev-env.sh') < m1.index('scripts/env.sh') < m1.index('docker compose')

    m2 = (root / 'deploy/scripts/m2-demo.sh').read_text()
    writer = m2[m2.index('make_env() {'):m2.index('\nmake_env\n')]
    script = 'root=$1; base=$1/var; envfile=$base/m2.env\n' + writer + '\nmake_env\n'
    subprocess.run(['bash', '-eu', '-c', script, 'fixture', str(work)],
                   env=dict(env, DEV_LAN_CIDR='192.0.2.0/24'), check=True)
    demo = keys(work / 'var/m2.env')
    assert demo['PORCHLIGHT_VERSION'] == version
    assert demo['ENROLL_CIDRS'] == '192.0.2.0/24,172.31.252.1/32,172.31.253.1/32'

    installer = (root / 'deploy/install.sh').read_text()
    server_writer = next(code for code in re.findall(r'python3 -I -B - "\$stackroot/\.env" "\$version" <<\'PY\'\n(.*?)\nPY', installer, re.S) if code.startswith('import os,secrets'))
    # Exercise the actual installed-stack writer with non-secret deterministic passwords.
    subprocess.run(['python3', '-I', '-', str(work / 'var/stack.env'), version],
                   input="import secrets\nsecrets.token_hex=lambda _: 'fixture-password'\n" + server_writer,
                   env=dict(env, ENROLL_CIDRS='127.0.0.1/32'), text=True, check=True)
    stack = keys(work / 'var/stack.env')
    contracts = {'compose.dev.yml': [dev, dict(dev, PORCHLIGHT_VERSION=version), demo],
                 'compose.yml': [stack],
                 # M9 override is fail-closed; install.sh funnel persists these operator inputs.
                 'compose.funnel.yml': [{'TAILSCALE_IMAGE_SHA256': 'a' * 64, 'TS_HOSTNAME': 'quiet-node'}]}
    files = list((root / 'deploy').rglob('*compose*.yml'))
    assert {p.name for p in files} == set(contracts), 'new compose file needs an env contract'
    for file in files:
        required = set(re.findall(r'\$\{([A-Z_][A-Z_0-9]*):\?', file.read_text()))
        for supplied in contracts[file.name]:
            assert all(supplied.get(key) for key in required), 'required Compose variable not provided: ' + file.name
    api = (root / 'deploy/compose.yml').read_text().split('  api:\n', 1)[1].split('  backup:\n', 1)[0]
    for key in ('PORCHLIGHT_RELEASE_REPO', 'PORCHLIGHT_RELEASE_PUBLIC_KEY', 'PORCHLIGHT_UPGRADE_COMMAND'):
        assert key + ': ${' + key + ':-}' in api
    assert 'PORCHLIGHT_RELEASE_DIR: /release' in api
    assert '${PORCHLIGHT_ROOT:-/var/lib/porchlight}/releases/${PORCHLIGHT_VERSION:?missing release version}:/release:ro' in api
    assert '/rollback' not in api
print('PASS all Compose required-variable contracts, env.sh and M1/M2/installed-stack writers')
