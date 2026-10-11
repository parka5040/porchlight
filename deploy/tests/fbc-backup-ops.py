#!/usr/bin/env python3
"""Execute real parser/cleanup logic with private fixtures, no Docker or emulator."""
import os
import pathlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
restore = (ROOT / 'deploy/scripts/restore-drill.sh').read_text()
gate = (ROOT / 'deploy/scripts/m8-acceptance.sh').read_text()
parser = restore[restore.index('keep=0 bundle='):restore.index('# age on this laptop')]
cleanup = gate[gate.index('cleanup() {'):gate.index('\ntrap cleanup EXIT')]
with tempfile.TemporaryDirectory(prefix='fbc-ops-', dir=ROOT / 'var') as fixture:
    base = pathlib.Path(fixture)
    (base / 'var/backups').mkdir(parents=True)
    (base / 'var/backups/porchlight-latest.tar.age').write_text('fixture ciphertext')
    (base / 'var/backup-recovery.key').write_text('not a real key')
    # If an explicit empty input wrongly selects the unrelated latest, this emits SELECTED.
    harness = 'set -euo pipefail\nroot=$1; shift\n' + parser + '\nprintf "SELECTED\\n"\n'
    for args in [('',), ('', ''), ('--keep', '')]:
        result = subprocess.run(['bash', '-c', harness, 'fixture', str(base), *args], capture_output=True, text=True)
        assert result.returncode == 2 and 'SELECTED' not in result.stdout, (args, result.returncode, result.stdout)
        assert 'explicit bundle path is empty' in result.stdout
    installer = base / 'install.sh'; installer.write_text('# trusted fixture\n')
    result = subprocess.run(['bash', '-c', harness, 'fixture', str(base), '--installer', str(installer)], capture_output=True, text=True)
    assert result.returncode == 0 and result.stdout == 'SELECTED\n', result.stdout
    missing = subprocess.run(['bash', '-c', harness, 'fixture', str(base), str(base / 'missing')], capture_output=True, text=True)
    assert missing.returncode == 1 and 'SELECTED' not in missing.stdout
    print('PASS restore parser: empty explicit inputs fail; omitted bundle selects latest; missing path fails')

    def run_cleanup(keep, attempted, original=0, uninstall=0):
        work = base / f'work-{keep}-{attempted}-{original}-{uninstall}'
        work.mkdir()
        trace = base / (work.name + '.trace')
        prelude = '''set -euo pipefail
work=$1; trace=$2; keep=$3; release_install_attempted=$4; original=$5; uninstall=$6
PORCHLIGHT_ROOT=$work/nonexistent; emulator_pid=fixture; serial=emulator-5580; stage=apk
# Record calls; do not talk to an AVD or kill a real process.
timeout() { shift; "$@"; }
adb() { printf '%s\\n' "$*" >> "$trace"; if [[ $* == *'uninstall app.porchlight' ]]; then return "$uninstall"; fi; }
kill() { return 1; }
wait() { return 0; }
pgrep() { return 1; }
reap_stale_netsimd() { :; }
'''
        result = subprocess.run(['bash', '-c', prelude + cleanup + '\ntrap cleanup EXIT\nexit "$original"\n',
                                 'fixture', str(work), str(trace), str(keep), str(attempted), str(original), str(uninstall)],
                                capture_output=True, text=True)
        calls = trace.read_text().splitlines()
        return result, calls, work

    for original in [0, 1, 130, 143]:
        result, calls, work = run_cleanup(0, 1, original)
        assert result.returncode == original, (original, result.returncode)
        assert calls == ['-s emulator-5580 uninstall app.porchlight', '-s emulator-5580 emu kill'], calls
        assert work.exists() == (original != 0)
    for keep, attempted in [(1, 1), (0, 0)]:
        result, calls, _ = run_cleanup(keep, attempted)
        assert result.returncode == 0 and calls == ['-s emulator-5580 emu kill'], calls
    result, calls, work = run_cleanup(0, 1, 0, 1)
    assert result.returncode == 1 and calls[-1].endswith('emu kill') and work.exists()
    install = gate.index('private apk adb -s "$serial" install -r')
    assert gate.rfind('release_install_attempted=1', 0, install) > gate.index('stage=apk')
    for name in ['passive-pull.md', 'backup-restore.md']:
        doc = (ROOT / 'docs/operations' / name).read_text()
        assert 'staged=$(python3 -I deploy/scripts/stage-pulled-bundle.py "$bundle" "$receipt") &&' in doc
    print('PASS M8 cleanup: uninstall before shutdown on success/failure/signals; keep/preinstall skip; uninstall failure fails gate')
