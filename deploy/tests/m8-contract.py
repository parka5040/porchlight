#!/usr/bin/env python3
"""Offline shape guard, not a substitute for the controller's actual acceptance run."""
import pathlib
import re
import subprocess
ROOT = pathlib.Path(__file__).resolve().parents[2]
script = ROOT / 'deploy/scripts/m8-acceptance.sh'
subprocess.run(['bash', '-n', str(script)], check=True)
text = script.read_text()
for required in ['--upgrade-source', 'version_code "$new"', 'mktemp -d "$root/var/', 'trap cleanup EXIT', '--no-funnel', '--no-backup',
                 'cli dev-init', 'cli items add', 'cli lists add', 'cli attach', 'cli comments add', 'cli export --household',
                 'verify-export.py', 'sha256sum --check', 'restore-drill.sh', 'install.sh" upgrade', 'post-attachment',
                 'JAVA_TOOL_OPTIONS="-Duser.home=', '--backup-pull-smoke', 'porch-api35', 'reap_stale_netsimd', 'adb -s "$serial" install -r']:
    assert required in text, f'missing acceptance hook: {required}'
for required in ['. scripts/emulator-common.sh', 'pgrep -u "$(id -u)" -x netsimd', 'apk-uninstall', 'uninstall app.porchlight', 'apksigner', 'verify --print-certs', 'apk-installed-signature', 'XDG_STATE_HOME=', 'server-backups.json', 'row[\'sha256\']', 'row[\'size\']']:
    assert required in text, f'missing fix-round hook: {required}'
assert 'reap_stale_netsimd()' not in text, 'use the shared emulator helper, not a local copy'
assert "fail 'shut down existing emulators first'" in text
assert "fail 'shut down starting emulators first'" in text
assert 'reap_stale_netsimd\npgrep -u "$(id -u)" -x netsimd >/dev/null && fail' in text
assert 'reap_stale_netsimd\n    if pgrep -u "$(id -u)" -x netsimd' in text
for path in (ROOT / 'docs/operations').glob('*.md'):
    doc = path.read_text()
    assert not re.search(r'\bdeploy/install\.sh[\s`]+restore\b', doc), f'template restore command in {path.name}'
for name in ['passive-pull.md', 'backup-restore.md']:
    doc = (ROOT / 'docs/operations' / name).read_text()
    assert 'bash dist/RECOVERY_VERSION/install.sh restore "$staged"' in doc
    assert '--root "$PWD/var/restore-target"' in doc
    assert 'target root must be under the repository' in doc
assert text.index('uninstall app.porchlight') < text.index('install -r')
assert 'rm -rf -- "$work" || code=1' in text
assert '"porchlight-backup:$active_version"' in text
for required in ['post-version', '/usr/local/bin/porchlight --version', "['entries']", 'listed-items.json', "['label']"]:
    assert required in text, f'missing functional assertion: {required}'
for index, snippet in enumerate(re.findall(r"<<'PY'\n(.*?)\nPY", text, re.S)):
    compile(snippet, f'm8-inline-{index}', 'exec')
assert 'set -x' not in text and 'deploy-server.sh' not in text and 'PORCHLIGHT_DEPLOY_' not in text  # laptop-only gate
print('PASS M8 gate syntax and controller hooks (live gate not run)')
