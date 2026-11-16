#!/usr/bin/env python3
"""Join one load run and overlapping host samples without hiding unmeasured gates."""
import argparse
import json
import math
from pathlib import Path
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import Histogram, private_file, runtime_dir, write_private


def merge_loads(loads):
    if len(loads) == 1: return loads[0]
    merged = {'classes': {}, 'phases': {}, 'statuses': {}, 'stream_results': [], 'aborted': False, 'failed': False,
              'started_at': min(r['started_at'] for r in loads), 'ended_at': max(r['ended_at'] for r in loads),
              'scenario': 'edit-fanout' if any(r.get('scenario') == 'edit-fanout' for r in loads) else 'combined'}
    labels = {r.get('target_label') for r in loads}
    if len(labels) != 1: raise ValueError('cannot merge different target labels')
    for i, load in enumerate(loads):
        suffix = f' [{load.get("scenario", "run")} {i+1}]'
        merged['classes'].update({k+suffix: v for k, v in load['classes'].items()})
        merged['phases'].update({k+suffix: v for k, v in load.get('phases', {}).items()})
        for code, n in load['statuses'].items(): merged['statuses'][code] = merged['statuses'].get(code, 0)+n
        merged['stream_results'] += load.get('stream_results', [])
        merged['aborted'] |= load.get('aborted', False)
        merged['failed'] |= load.get('failed', False)
    comparisons = [r['fanout_compare'] for r in loads if 'fanout_compare' in r]
    if comparisons:
        merged['fanout_compare'] = {}
        for name in ('legacy', 'targeted'):
            variants = [c.get(name, {}) for c in comparisons]
            merged['fanout_compare'][name] = {
                'status': 'AVAILABLE' if all(v.get('status') == 'AVAILABLE' for v in variants) else 'NOT AVAILABLE',
                'edits': [e for v in variants for e in v.get('edits', [])],
                'requested_edits': sum(v.get('requested_edits', len(v.get('edits', []))) for v in variants),
                'complete': all(v.get('complete', True) for v in variants)}
    for key in ['sse_latency', 'sse_request_to_event']:
        hist = Histogram()
        for load in loads:
            row = load.get(key, {})
            if row.get('count'):
                h = Histogram.from_dict(row)
                hist.buckets.update(h.buckets)
                hist.count += h.count
                hist.maximum = max(hist.maximum, h.maximum)
        merged[key] = hist.as_dict()
    return merged


def percentile(values, p):
    return sorted(values)[max(0, math.ceil(len(values)*p/100)-1)] if values else None


def comparison_table(comparison):
    lines = ['', '## Fanout comparison', '',
             '| Protocol | Edits | requests/edit | wire MB/edit | decoded MB/edit | follow-up completion p50 ms | follow-up completion p95 ms |',
             '|---|---:|---:|---:|---:|---:|---:|']
    for name in ('legacy', 'targeted'):
        variant = comparison.get(name, {})
        if variant.get('status') != 'AVAILABLE':
            lines.append(f'| {name} | NOT AVAILABLE | — | — | — | — | — |')
            continue
        edits = variant.get('edits', [])
        complete = [e for e in edits if e.get('all_devices_current_ms') is not None]
        n = len(complete)
        if not n:
            lines.append(f'| {name} | 0 (INCOMPLETE) | — | — | — | — | — |')
            continue
        means = [sum(e['total'][k] for e in complete)/n/divisor for k, divisor in
                 [('requests', 1), ('wire_bytes', 1000000), ('decoded_bytes', 1000000)]]
        times = [e['all_devices_current_ms'] for e in complete]
        values = means + [percentile(times, 50), percentile(times, 95)]
        lines.append(f'| {name} | {n}/{variant.get("requested_edits", len(edits))} | '+' | '.join(f'{x:.3f}' for x in values)+' |')
    lines += ['', 'Totals include every device, writer/version reads and delivered SSE entity bytes; stream setup is separate in JSON.',
              'MB = 1,000,000 bytes. Wire means compressed HTTP response body bytes, not TLS, headers or request bodies.',
              'Completion is request-start → last follow-up read across all devices (conservative commit upper bound).',
              'A post-read quiet period checks for more frames but is excluded from latency. Quiescence is not a server-side completion fence.',
              'Per-device counts, bytes, frame latency bounds and incomplete edits remain in the raw JSON.']
    return lines


def render(load, observe, budgets):
    lines = ['# Porchlight performance measurement', '', '| Budget | Result | Observed | Limit |', '|---|---|---:|---:|']
    results = []
    def check(name, observed, limit, exclusive=False):
        verdict = 'NOT MEASURED' if observed is None else ('PASS' if (observed < limit if exclusive else observed <= limit) else 'FAIL')
        results.append(verdict)
        value = '—' if observed is None else f'{observed:.3f}'
        lines.append(f'| {name} | {verdict} | {value} | {"<" if exclusive else "≤"} {limit} |')
    classes = dict(load.get('classes', {}))
    for phase, rows in load.get('phases', {}).items():
        for name, stats in rows.items():
            classes[name+' ['+phase+']'] = stats
    for kind, keys in [('read.', [('p95_ms', 'read_p95_ms'), ('p99_ms', 'read_p99_ms')]),
                       ('write.', [('p95_ms', 'write_p95_ms')]), ('heavy.', [('p95_ms', 'heavy_edit_p95_ms')])]:
        selected = {k: v for k, v in classes.items() if k.startswith(kind)}
        if not selected:
            for _, budget in keys: check(kind+budget, None, budgets[budget])
        for cls, stats in selected.items():
            for field, budget in keys: check(cls+' '+field, stats.get(field), budgets[budget])
    # Conservative request-start bound is preferable; ack bound is explicitly labeled for old fixtures.
    sse = load.get('sse_request_to_event') or load.get('sse_latency', {})
    check('write→SSE p95 ms (request-start upper bound)', sse.get('p95_ms') if sse.get('count') else None, budgets['sse_p95_ms'])
    times = [r for r in observe if load.get('started_at', 0) <= r['time'] <= load.get('ended_at', float('inf'))]
    for service, budget in [('api', 'api_memory_bytes_exclusive'), ('postgres', 'postgres_memory_bytes_exclusive')]:
        values = [c['memory_bytes'] for r in times for c in r.get('containers', []) if c['service'] == service]
        check(service+' peak memory bytes', max(values) if values else None, budgets[budget], exclusive=True)
    statuses = load.get('statuses', {})
    total = sum(statuses.values())
    errors = sum(v for k, v in statuses.items() if k.isdigit() and 500 <= int(k) <= 599)
    check('HTTP 5xx fraction', errors/total if total else None, budgets['http_5xx_fraction_exclusive'], exclusive=True)
    rejected = sum(v for k, v in statuses.items() if k.isdigit() and 400 <= int(k) <= 499)
    check('HTTP 4xx (workload validity, including conflicts/rate limits)', rejected if total else None, 0)
    counters = [sum(c.get('oom', 0)+c.get('oom_kill', 0)+c.get('oom_group_kill', 0) for c in r.get('oom', {}).values()) for r in times if r.get('oom')]
    restarts = {}
    for row in times:
        for service, c in row.get('oom', {}).items():
            if 'restarts' in c: restarts.setdefault(service, []).append(c['restarts'])
    restart_delta = sum(max(v)-min(v) for v in restarts.values()) if restarts else None
    check('container restarts during run (OOM kills reset cgroup counters)', restart_delta, 0)
    peaks = {s: max(c.get('memory_peak_bytes', 0) for row in times for name, c in row.get('oom', {}).items() if name == s) for s in restarts}
    # Absolute counters conservatively fail if an OOM preceded the run; observer does not reset cgroups.
    oom_covered = all(any(service in r.get('oom', {}) for r in times) for service in ('api', 'postgres'))
    check('OOM counters (absolute, conservative)', max(counters) if counters and (oom_covered or max(counters) > 0) else None, budgets['oom_allowed'])
    transport = statuses.get('timeout/transport', 0)
    check('transport errors', transport if total else None, 0)
    check('safety abort / execution failure', int(load.get('aborted', False) or load.get('failed', False)), 0)
    missed = sum(not r['received'] for r in load.get('stream_results', []) if 'received' in r)
    comparison = load.get('fanout_compare')
    if comparison is not None:
        missed += sum(not d.get('received', False) for v in comparison.values() for e in v.get('edits', []) for d in e.get('devices', []))
    check('missing fanout deliveries', missed if load.get('scenario') == 'edit-fanout' or comparison is not None else None, 0)
    if comparison is not None:
        for name in ('legacy', 'targeted'):
            v = comparison.get(name, {})
            edits = v.get('edits', [])
            completed = sum(e.get('all_devices_current_ms') is not None for e in edits)
            valid = v.get('status') == 'AVAILABLE'
            check(name+' unfinished edits', v.get('requested_edits', len(edits))-completed if valid else None, 0)
        targeted = comparison.get('targeted', {})
        edits = targeted.get('edits', []) if targeted.get('status') == 'AVAILABLE' else []
        wire = [d['wire_bytes'] for e in edits for d in e.get('devices', []) if e.get('all_devices_current_ms') is not None]
        completion_times = [e['all_devices_current_ms'] for e in edits if e.get('all_devices_current_ms') is not None]
        check('targeted wire bytes/device/edit p95', percentile(wire, 95), budgets['targeted_wire_bytes_per_device_edit_p95'])
        check('targeted all devices current p95 ms', percentile(completion_times, 95), budgets['targeted_all_devices_current_p95_ms'])
    overall = 'FAIL' if 'FAIL' in results else ('INCOMPLETE' if 'NOT MEASURED' in results else 'PASS')
    lines += ['', '**Overall: '+overall+'**. Unmeasured budgets are not a pass.', '',
              'Latency histograms use upper-rounded 1% logarithmic buckets. HTTP time includes full body consumption.',
              'The server does not expose a commit timestamp: request-start→event is an upper bound; ack→event a lower bound.',
              'Mix percentages describe logical operations; auxiliary version reads consume the same HTTP rate cap.',
              'Memory/OOM samples overlap the load time window. Missing cgroup/statements permission is not zero usage.', '',
              '## Observer coverage', f'- Overlapping samples: {len(times)}',
              '- Unavailable fields: '+', '.join(sorted({s for r in times for s in r.get('unavailable', [])})),
              '', '## Status codes', '```json', json.dumps(statuses, sort_keys=True), '```', '', '## Host maxima']
    for key in ['longest_transaction_seconds', 'lock_waits', 'outbox_backlog', 'pending_derivations', 'database_bytes']:
        values = [r['postgres'][key] for r in times if key in r.get('postgres', {})]
        lines.append(f'- {key}: {max(values) if values else "NOT MEASURED"}')
    lines += ['', '## Container peaks']
    lines += [f'- cgroup memory.peak {s}: {peaks[s]/1048576:.1f} MiB (since container start)' for s in sorted(peaks) if peaks[s]]
    lines += ['| Service | CPU % | Memory MiB | PIDs |', '|---|---:|---:|---:|']
    services = sorted({c['service'] for r in times for c in r.get('containers', [])})
    for service in services:
        rows = [c for r in times for c in r.get('containers', []) if c['service'] == service]
        lines.append(f'| {service} | {max(c.get("cpu_percent", 0) for c in rows):.2f} | {max(c["memory_bytes"] for c in rows)/1048576:.2f} | {max(c.get("pids", 0) for c in rows)} |')
    states = {}
    for r in times:
        for state, count in r.get('postgres', {}).get('connections', {}).items():
            states[state] = max(states.get(state, 0), count)
    lines += ['', '## Connection-state peaks', '```json', json.dumps(states, sort_keys=True), '```', '',
              '## Latest top statements (ranked by total execution time; no query text)',
              '| Query ID | Calls | Mean ms | Max ms | Rows |', '|---:|---:|---:|---:|---:|']
    samples = [r['postgres']['statements'] for r in times if isinstance(r.get('postgres', {}).get('statements'), list)]
    if samples:
        for s in samples[-1][:20]:
            lines.append(f'| {int(s["queryid"])} | {int(s["calls"])} | {float(s["mean_exec_time"]):.3f} | {float(s["max_exec_time"]):.3f} | {int(s["rows"])} |')
    else: lines.append('| NOT MEASURED | | | | |')
    if comparison is not None: lines += comparison_table(comparison)
    return '\n'.join(lines)+'\n'


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--load', required=True, action='append', help='repeat for a complete multi-scenario budget report')
    p.add_argument('--observe', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--budgets', default=str(Path(__file__).with_name('budgets.json')))
    a = p.parse_args()
    load = merge_loads([json.loads(private_file(path).read_text()) for path in a.load])
    observe = [json.loads(line) for line in private_file(a.observe).read_text().splitlines() if line]
    text = render(load, observe, json.loads(Path(a.budgets).read_text()))
    write_private(runtime_dir(a.out), text)
    print(next(line for line in text.splitlines() if line.startswith('**Overall:')))
    if '**Overall: FAIL**' in text: sys.exit(1)
    if '**Overall: INCOMPLETE**' in text: sys.exit(2)

if __name__ == '__main__':
    try: main()
    except Exception:
        print('FAIL: report input refused or invalid', file=sys.stderr)
        sys.exit(1)
