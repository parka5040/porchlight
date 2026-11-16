#!/usr/bin/env python3
"""Host-only fixed-query Docker/Postgres observer. Never records SQL text or parameters."""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import runtime_dir

QUERIES = {
    'connections': "select coalesce(json_object_agg(state,n),'{}'::json) from (select coalesce(state,'unknown') state,count(*) n from pg_stat_activity group by state) s",
    'longest_transaction_seconds': "select coalesce(max(extract(epoch from now()-xact_start)),0) from pg_stat_activity where pid<>pg_backend_pid()",
    'lock_waits': 'select count(*) from pg_locks where not granted',
    'statements': "select coalesce(json_agg(s),'[]'::json) from (select queryid,calls,mean_exec_time,max_exec_time,rows from pg_stat_statements order by total_exec_time desc limit 20) s",
    'outbox_backlog': 'select count(*) from porchlight.outbox where stream_seq is null',
    'pending_derivations': 'select count(*) from porchlight.item_derivation where applied_gen < requested_gen',
    'database_bytes': 'select pg_database_size(current_database())',
}


def command(args):
    try:
        r = subprocess.run(args, capture_output=True, text=True, timeout=15)
        return r.stdout.strip() if r.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired): return None


def memory_bytes(text):
    m = re.fullmatch(r'([\d.]+)\s*(B|[KMGT]i?B)', text.strip())
    if not m: raise ValueError('unknown docker memory unit')
    unit = m[2]
    base = 1024 if 'i' in unit else 1000
    exponent = 'BKMGT'.index(unit[0]) if unit != 'B' else 0
    return round(float(m[1])*base**exponent)


def oom_counters(container):
    # Narrow inspect only; neither Config.Env nor any other object is fetched. A kernel OOM kill restarts the
    # container and recreates its cgroup (counters reset), so RestartCount is the durable signal; memory.peak
    # catches spikes between 5 s samples.
    state = command(['docker', 'inspect', '--format', '{{.State.Pid}} {{.RestartCount}}', container])
    if not state or not re.fullmatch(r'\d+ \d+', state): return None
    pid, restarts = state.split()
    try:
        for line in Path('/proc', pid, 'cgroup').read_text().splitlines():
            if line.startswith('0::'):
                base = Path('/sys/fs/cgroup')
                scope = (base / line[3:].lstrip('/')).resolve()
                if not scope.is_relative_to(base): return None
                counters = {k: int(v) for k, v in (r.split() for r in (scope / 'memory.events').read_text().splitlines()) if k in ('oom', 'oom_kill', 'oom_group_kill')}
                counters['restarts'] = int(restarts)
                try: counters['memory_peak_bytes'] = int((scope / 'memory.peak').read_text())
                except (OSError, ValueError): pass
                return counters
    except (OSError, ValueError): pass
    return None


def sample(project, pg, database):
    row = {'time': time.time(), 'containers': [], 'oom': {}, 'postgres': {}, 'unavailable': []}
    raw = command(['docker', 'ps', '--filter', 'label=com.docker.compose.project='+project,
                   '--format', '{{.ID}}\t{{.Label "com.docker.compose.service"}}'])
    if raw is None:
        row['unavailable'].append('docker')
        return row
    for line in raw.splitlines():
        container, _, service = line.partition('\t')
        stats = command(['docker', 'stats', '--no-stream', '--format', '{{json .}}', container])
        if stats:
            try:
                value = json.loads(stats)
                row['containers'].append({'service': service, 'cpu_percent': float(value['CPUPerc'].rstrip('%')),
                                          'memory_bytes': memory_bytes(value['MemUsage'].split('/')[0]), 'pids': int(value['PIDs'])})
            except (ValueError, KeyError): row['unavailable'].append('container-stats')
        else: row['unavailable'].append('container-stats')
        counters = oom_counters(container)
        if counters is not None: row['oom'][service] = counters
        else: row['unavailable'].append('cgroup-oom:'+service)
    for name, sql in QUERIES.items():
        result = command(['docker', 'exec', pg, 'psql', '-U', 'postgres', '-d', database,
                          '-XAt', '-v', 'ON_ERROR_STOP=1', '-c', sql])
        if result is None: row['unavailable'].append(name)
        else:
            try: row['postgres'][name] = json.loads(result)
            except ValueError: row['unavailable'].append(name)
    return row


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--project', required=True)
    p.add_argument('--pg-container', required=True)
    p.add_argument('--database', default='porchlight')
    p.add_argument('--out', required=True)
    p.add_argument('--duration', type=int, default=300)
    a = p.parse_args()
    if not 1 <= a.duration <= 7200: raise ValueError('observer duration cap 7200 s')
    if not re.fullmatch(r'[a-zA-Z0-9_.-]+', a.project+a.pg_container+a.database): raise ValueError('invalid Docker/database identifier')
    out = runtime_dir(a.out)
    out.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(out, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    stopping = False
    def stop(signum, frame):
        nonlocal stopping
        stopping = True
    # Finish the current JSONL record on the controller's kill/interrupt, never leave a partial line.
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    with os.fdopen(fd, 'a') as f:
        os.fchmod(f.fileno(), 0o600)
        end = time.monotonic()+a.duration
        tick = time.monotonic()
        while time.monotonic() < end and not stopping:
            f.write(json.dumps(sample(a.project, a.pg_container, a.database), sort_keys=True)+'\n')
            f.flush()
            os.fsync(f.fileno())
            tick += 5
            time.sleep(max(0, min(tick, end)-time.monotonic()))

if __name__ == '__main__':
    try: main()
    except KeyboardInterrupt: pass
    except Exception:
        print('FAIL: observer refused or failed; no raw diagnostics emitted', file=sys.stderr)
        sys.exit(1)
