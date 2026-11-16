#!/usr/bin/env python3
"""Synthetic household through porch only; durable intent/reconciliation journal."""
import argparse
import datetime as dt
import fcntl
import json
import math
import os
from pathlib import Path
import random
import secrets
import subprocess
import struct
import sys
import time
import zlib

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import digest, private_file, runtime_dir, write_private

NAMES = ['Sam Rivera', 'Alex Kim', 'Jordan Lee', 'Taylor Morgan', 'Casey Chen', 'Robin Patel']
GAPS = [
    'Complete/reopen and set-status skipped use server time: porch has no completed-at/backdate option.',
    'Derivation bounds/calendar materialization caps mean five years of daily history is not guaranteed.',
    'Historic available occurrences are genuinely completed/checked now, not at their nominal dates.',
    'No CLI idempotency-key override: ambiguous outcomes are reconciled by deterministic labels/exports.',
]


def make_plan(tier, scale, seed, anchor):
    if not math.isfinite(scale) or not 0 < scale <= 10:
        raise ValueError('scale must be >0 and <=10')
    day = dt.date.fromisoformat(anchor)
    factor = 10 if tier == 'mature' else 1
    n = lambda base: max(1, round(base * scale))
    counts = {'members': 6, 'items': n(2000 * factor), 'series': n(100 * factor),
              'comments': n(5000 * factor), 'lists': n(50 * factor), 'entries': n(1000 * factor),
              'attachments': n(1000 if tier == 'mature' else 200)}
    counts['series'] = min(counts['items'], counts['series'])
    rng = random.Random(seed)
    prefix = f'perf-{seed}-{tier}'
    steps = []
    def add(key, kind, args, label):
        steps.append({'key': key, 'kind': kind, 'args': args, 'label': label})
    for i, name in enumerate(NAMES):
        user = f'{prefix}-member-{i}'
        add(f'member{i}', 'member', ['users', 'add', '--display-name', name, '--username', user, '--tz', 'America/New_York'], user)
    for i in range(counts['items']):
        label = f'{prefix}-item-{i:06d}'
        args = ['items', 'add', '--title', label, '--notes', f'Synthetic fixture {rng.randrange(1000000)}',
                '--owner', f'$member{i % 6}', '--assign', f'$member{i % 6}', '--visibility', 'household']
        when = day + dt.timedelta(days=rng.randrange(-180, 549))
        if i < counts['series']:
            # Daily majority gives roughly 20k occurrences initially, plus weekly/monthly and completion series.
            mode = i % 10
            when = day - dt.timedelta(days=180)
            if mode < 2: args += ['--rrule', 'FREQ=WEEKLY;COUNT=104']
            elif mode == 2: args += ['--rrule', 'FREQ=MONTHLY;COUNT=24']
            elif mode == 3: args += ['--every', '1d']
            else: args += ['--rrule', 'FREQ=DAILY;COUNT=300']
            args += ['--missed', 'stack'] if mode != 3 else []
        if i % 3 == 0:
            args += ['--due', when.isoformat() + 'T09:00:00', '--tz', 'America/New_York']
        elif i % 3 == 1:
            args += ['--due', when.isoformat() + 'T09:00:00', '--floating']
        else:
            args += ['--all-day', '--date', when.isoformat(), '--tz', 'America/New_York']
        add(f'item{i}', 'item', args, label)
    for i in range(counts['lists']):
        label = f'{prefix}-list-{i:05d}'
        add(f'list{i}', 'list', ['lists', 'add', label, '--owner-id', '$member0'], label)
    for i in range(counts['entries']):
        label = f'{prefix}-entry-{i:06d}'
        add(f'entry{i}', 'entry', ['lists', 'add-entry', f'$list{i % counts["lists"]}', label], label)
    for i in range(counts['comments']):
        label = f'{prefix}-comment-{i:06d} @Sam Rivera synthetic note'
        add(f'comment{i}', 'comment', ['comments', 'add', f'$item{i % counts["items"]}', label], label)
    for i in range(counts['attachments']):
        extension = 'png' if i % 2 else 'txt'
        label = f'{prefix}-attachment-{i:05d}.{extension}'
        add(f'attachment{i}', 'attachment', ['attach', f'$item{i % counts["items"]}', label], label)
    if tier == 'mature':
        label = f'{prefix}-five-year-daily'
        start = (day - dt.timedelta(days=1825)).isoformat()
        add('history', 'item', ['items', 'add', '--title', label, '--owner', '$member0', '--assign', '$member0',
                               '--due', start+'T09:00:00', '--tz', 'America/New_York', '--rrule', 'FREQ=DAILY;COUNT=1825', '--missed', 'stack'], label)
        label = f'{prefix}-daily-checklist'
        add('historylist', 'list', ['lists', 'add', label, '--item-id', '$history', '--mode', 'per-occurrence'], label)
        for i in range(10):
            label = f'{prefix}-daily-check-{i}'
            add(f'historyentry{i}', 'entry', ['lists', 'add-entry', '$historylist', label], label)
        counts.update(items=counts['items']+1, series=counts['series']+1, lists=counts['lists']+1, entries=counts['entries']+10,
                      history_requested=1825, checks_requested=1825)
    return {'schema': 1, 'tier': tier, 'scale': scale, 'seed': seed, 'anchor': anchor, 'prefix': prefix,
            'counts': counts, 'steps': steps, 'gaps': GAPS}


class Porch:
    def __init__(self, binary, socket, token):
        self.command = [binary, '--socket', str(socket), '--token-file', str(private_file(token)), '--json']

    def __call__(self, args, password=None):
        env = dict(os.environ)
        if password is not None: env['PORCHLIGHT_PASSWORD'] = password
        # Raw replies/problems can contain credentials: never relay subprocess output.
        r = subprocess.run(self.command + list(args), env=env, capture_output=True, text=True, timeout=950)
        if r.returncode: raise ValueError('porch failed; no raw diagnostics emitted; rerun to reconcile')
        return json.loads(r.stdout)

    def pages(self, args):
        rows, seen, cursor = [], set(), None
        while True:
            page = self(args + (['--cursor', cursor] if cursor else []))
            rows.extend(page['items'])
            cursor = page.get('next_cursor')
            if not cursor: return rows
            if cursor in seen: raise ValueError('repeated porch cursor')
            seen.add(cursor)


def journal_read(path):
    if not path.exists(): return []
    private_file(path)
    rows = []
    raw = path.read_bytes()
    lines = raw.splitlines(keepends=True)
    for i, line in enumerate(lines):
        try: rows.append(json.loads(line))
        except ValueError:
            if i != len(lines)-1 or line.endswith(b'\n'): raise ValueError('corrupt journal') from None
            # Only an incomplete trailing write may be discarded, never a completed intent.
            with path.open('r+b') as f: f.truncate(sum(map(len, lines[:i])))
    return rows


def append(path, row):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600), 'a') as f:
        os.fchmod(f.fileno(), 0o600)
        f.write(json.dumps(row, sort_keys=True)+'\n')
        f.flush()
        os.fsync(f.fileno())


def export_tables(porch, out, member):
    p = out / 'inventory.json'
    if p.exists():
        private_file(p)
        p.unlink()
    porch(['export', '--member', member, '--out', str(p)])
    private_file(p)
    data = json.loads(p.read_text())
    return data.get('tables', data)


def apply(plan, out, porch):
    path = out / 'journal.jsonl'
    rows = journal_read(path)
    h = digest(plan)
    if rows and rows[0] != {'plan_hash': h}: raise ValueError('plan changed: use same seed/scale/anchor or a new output directory')
    if not rows: append(path, {'plan_hash': h})
    done = {r['key']: r['result'] for r in rows if r.get('state') == 'done'}
    refs = {key: value['id'] for key, value in done.items() if 'id' in value}
    caches = {}
    def resolve(args): return [refs[x[1:]] if x.startswith('$') else x for x in args]
    def find(step, args):
        kind = step['kind']
        if kind in ('member', 'item', 'list'):
            cmds = {'member': ['users', 'list'], 'item': ['items', 'list'], 'list': ['lists', 'list']}
            if kind not in caches:
                caches[kind] = porch.pages(cmds[kind]) if kind == 'list' else porch(cmds[kind])['items']
            field = {'member': 'username', 'item': 'title', 'list': 'name'}[kind]
            found = [r for r in caches[kind] if r.get(field) == step['label']]
        elif kind == 'entry':
            key = ('entries', args[2])
            if key not in caches:
                detail = porch(['lists', 'show', args[2]])
                caches[key] = detail['entries'] + detail.get('got_it', [])
            found = [r for r in caches[key] if r['label'] == step['label']]
        elif kind == 'comment':
            key = ('comments', args[2])
            if key not in caches: caches[key] = porch.pages(['comments', 'list', args[2], '--limit', '200'])
            found = [r for r in caches[key] if r['body'] == step['label']]
        else:
            # Export contains attachment metadata; porch has no attachment-list command.
            if 'attachments' not in caches:
                caches['attachments'] = export_tables(porch, out, refs['member0']).get('attachments', [])
            found = [r for r in caches['attachments'] if r['original_name'] == step['label'] and not r.get('deleted_at')]
        if len(found) > 1: raise ValueError('duplicate deterministic labels; use an empty synthetic household')
        return found[0] if found else None
    for step in plan['steps']:
        key = step['key']
        if key in done: continue
        args = resolve(step['args'])
        password = None
        if step['kind'] == 'member':
            p = out / 'passwords' / (step['label'] + '.password')
            if not p.exists(): write_private(p, secrets.token_urlsafe(30))
            password = private_file(p).read_text()
        # Always reconcile labels, including when the journal was lost but the server survived.
        found = find(step, args)
        if found is None:
            append(path, {'state': 'intent', 'key': key})
            if step['kind'] == 'attachment':
                attachment = out / 'attachments' / step['label']
                content = ('Synthetic household notes: '+step['label']+'\n') * 32
                if attachment.suffix == '.png':
                    def chunk(kind, data):
                        return struct.pack('!I', len(data))+kind+data+struct.pack('!I', zlib.crc32(kind+data))
                    pixel = bytes.fromhex(digest(step['label'])[:6])
                    content = b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR', struct.pack('!IIBBBBB', 1, 1, 8, 2, 0, 0, 0))+chunk(b'IDAT', zlib.compress(b'\x00'+pixel))+chunk(b'IEND', b'')
                write_private(attachment, content)
                args[-1] = str(attachment)
            found = porch(args, password)
            # Invalidate only the relevant cached parent collection after a mutation.
            if step['kind'] in ('member', 'item', 'list'): caches[step['kind']].append(found)
            elif step['kind'] == 'entry': caches[('entries', args[2])].append(found)
            elif step['kind'] == 'comment': caches[('comments', args[2])].append(found)
            else: caches['attachments'].append(found)
        append(path, {'state': 'done', 'key': key, 'result': found})
        done[key] = found
        refs[key] = found['id']
    # Real completion/check writes, never SQL or fake old completion timestamps.
    if plan['tier'] == 'mature':
        available = []
        for _ in range(120):
            available = porch(['occurrences', 'list', refs['history'], '--from',
                               (dt.date.fromisoformat(plan['anchor'])-dt.timedelta(days=1826)).isoformat()+'T00:00:00Z',
                               '--to', plan['anchor']+'T00:00:00Z'])['items']
            if available: break
            time.sleep(1)
        for i, occ in enumerate(available[:1825]):
            key = 'complete:'+occ['id']
            if key not in done:
                if occ.get('status') != 'done': porch(['occurrences', 'complete', occ['id'], '--version', str(occ['version'])])
                append(path, {'state': 'done', 'key': key, 'result': {}})
            check = 'check:'+occ['id']
            if check not in done:
                porch(['lists', 'check', refs[f'historyentry{i % 10}'], '--list-id', refs['historylist'], '--occurrence-id', occ['id']])
                append(path, {'state': 'done', 'key': check, 'result': {}})
        write_private(out / 'history-coverage.json', json.dumps({'requested': 1825, 'available': len(available), 'gaps': GAPS}, indent=2))
    write_private(out / 'manifest.json', json.dumps({'plan_hash': h, 'prefix': plan['prefix'], 'refs': refs, 'counts': plan['counts'], 'gaps': GAPS}, indent=2))
    return {'steps_done': len(plan['steps']), 'plan_hash': h, 'gaps': GAPS}


def verify(plan, out, porch):
    rows = journal_read(out / 'journal.jsonl')
    done = {r['key']: r['result'] for r in rows if r.get('state') == 'done'}
    counts = {'members': 0, 'items': 0, 'lists': 0, 'entries': 0, 'comments': 0, 'attachments': 0, 'occurrences': 0}
    if not done: raise ValueError('apply before verify')
    remote = {'member': porch(['users', 'list'])['items'], 'item': porch(['items', 'list'])['items'],
              'list': porch.pages(['lists', 'list'])}
    tables = export_tables(porch, out, done['member0']['id'])
    remote['attachment'] = tables.get('attachments', [])
    fields = {'member': 'username', 'item': 'title', 'list': 'name', 'entry': 'label', 'comment': 'body', 'attachment': 'original_name'}
    mapped = {'member': 'members', 'item': 'items', 'list': 'lists', 'entry': 'entries', 'comment': 'comments', 'attachment': 'attachments'}
    for step in plan['steps']:
        kind = step['kind']
        if kind == 'entry':
            parent = done[step['args'][2][1:]]['id']
            if ('entry', parent) not in remote:
                detail = porch(['lists', 'show', parent])
                remote[('entry', parent)] = detail['entries'] + detail.get('got_it', [])
            found = remote[('entry', parent)]
        elif kind == 'comment':
            parent = done[step['args'][2][1:]]['id']
            if ('comment', parent) not in remote: remote[('comment', parent)] = porch.pages(['comments', 'list', parent, '--limit', '200'])
            found = remote[('comment', parent)]
        else: found = remote[kind]
        if sum(r.get(fields[kind]) == step['label'] and not r.get('deleted_at') for r in found) != 1:
            raise ValueError('verification missing/duplicate synthetic label')
        counts[mapped[kind]] += 1
    ids = {v['id'] for k, v in done.items() if k.startswith('item') or k == 'history'}
    counts['series'] = sum(r['id'] in ids and r.get('recurrence_mode') is not None for r in remote['item'])
    counts['occurrences'] = sum(r.get('item_id') in ids for r in tables.get('item_occurrences', []))
    counts['real_completions'] = sum(r.get('item_id') in ids and r.get('status') == 'done' for r in tables.get('item_occurrences', []))
    entry_ids = {v['id'] for k, v in done.items() if k.startswith(('entry', 'historyentry'))}
    counts['checks'] = sum(r.get('entry_id') in entry_ids for r in tables.get('list_entry_checks', []))
    write_private(out / 'verified.json', json.dumps({'counts': counts, 'gaps': GAPS}, indent=2))
    return {'counts': counts, 'gaps': GAPS}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('command', choices=['plan', 'apply', 'verify'])
    p.add_argument('--tier', choices=['initial', 'mature'], default='initial')
    p.add_argument('--scale', type=float, default=1)
    p.add_argument('--seed', type=int, default=42)
    p.add_argument('--anchor', default='2026-11-13', help='fixed ISO day; keep identical on resume')
    p.add_argument('--out', required=True)
    p.add_argument('--porch', default='porch')
    p.add_argument('--socket')
    p.add_argument('--token-file')
    a = p.parse_args()
    out = runtime_dir(a.out)
    plan = make_plan(a.tier, a.scale, a.seed, a.anchor)
    if a.command == 'plan':
        print(json.dumps({'counts': plan['counts'], 'plan_hash': digest(plan)}, sort_keys=True))
        return
    if not a.socket or not a.token_file: raise ValueError('socket and token-file required')
    porch = Porch(a.porch, a.socket, a.token_file)
    out.mkdir(parents=True, exist_ok=True, mode=0o700)
    # One process owns reconciliation/journaling for this output directory.
    lock = os.open(out, os.O_RDONLY | os.O_DIRECTORY)
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        print(json.dumps(apply(plan, out, porch) if a.command == 'apply' else verify(plan, out, porch), sort_keys=True))
    finally:
        os.close(lock)

if __name__ == '__main__':
    try: main()
    except Exception:
        print('FAIL: seed operation refused or failed (raw CLI output suppressed)', file=sys.stderr)
        sys.exit(1)
