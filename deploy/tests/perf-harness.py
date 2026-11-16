#!/usr/bin/env python3
"""Offline performance harness contracts; no service or credentials required."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'deploy/scripts/perf'))

def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'deploy/scripts/perf' / (name + '.py'))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m

common = module('common')
seed = module('seed-household')
load = module('load-household')
report = module('report')
observe = module('observe')

class Harness(unittest.TestCase):
    def setUp(self):
        (ROOT / 'var').mkdir(exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT / 'var')
        self.addCleanup(self.tmp.cleanup)
        self.out = Path(self.tmp.name)

    def test_plan_deterministic(self):
        a = seed.make_plan('initial', 0.002, 42, '2026-11-13')
        self.assertEqual(a, seed.make_plan('initial', 0.002, 42, '2026-11-13'))
        self.assertNotEqual(common.digest(a), common.digest(seed.make_plan('initial', 0.002, 43, '2026-11-13')))
        full = seed.make_plan('initial', 1, 42, '2026-11-13')
        self.assertEqual(full['counts']['items'], 2000)
        self.assertEqual(full['counts']['series'], 100)
        self.assertEqual(full['counts']['entries'], 1000)
        mature = seed.make_plan('mature', 0.01, 42, '2026-11-13')
        self.assertEqual(mature['counts']['series'], 11)
        self.assertEqual(mature['counts']['items'], 201)
        self.assertEqual(mature['counts']['checks_requested'], 1825)

    def test_guards(self):
        with self.assertRaises(ValueError): common.runtime_dir(ROOT / 'deploy/not-var')
        with self.assertRaises(ValueError): common.private_file(self.out / 'missing')
        token = self.out / 'token'
        token.write_text('fixture')
        token.chmod(0o644)
        with self.assertRaises(ValueError): common.private_file(token)
        token.chmod(0o600)
        self.assertEqual(common.private_file(token), token)
        link = self.out / 'link'
        link.symlink_to(token)
        with self.assertRaises(ValueError): common.private_file(link)
        with self.assertRaises(ValueError): load.validate_caps(51, 4, 30)
        with self.assertRaises(ValueError): load.validate_caps(5, 257, 30)
        with self.assertRaises(ValueError): load.validate_caps(5, 4, 3601)
        cfg = {'target_label': 'fixture', 'base_url': 'https://stack.invalid:8443'}
        with self.assertRaises(ValueError): load.validate_target(cfg, 'other', cfg['base_url'])
        with self.assertRaises(ValueError): load.validate_target(cfg, 'fixture', 'https://wrong.invalid')
        load.validate_target(cfg, 'fixture', cfg['base_url'])

    def test_percentiles(self):
        h = common.Histogram()
        for x in range(1, 101): h.add(x)
        s = h.summary()
        self.assertGreaterEqual(s['p95_ms'], 95)
        self.assertLess(s['p95_ms'], 96)
        self.assertEqual(s['max_ms'], 100)
        self.assertIsNone(common.Histogram().summary()['p95_ms'])
        self.assertEqual(common.Histogram.from_dict(h.as_dict()).summary(), s)

    def test_sse_bytes(self):
        p = common.SSEParser()
        fixture = b': heartbeat\r\n\r\nid: 7\r\nevent: item.changed\r\ndata: {"entity_id":\r\ndata: "fixture"}\r\n\r\n'
        frames = []
        for b in fixture: frames.extend(p.feed(bytes([b])))
        self.assertEqual(frames[0]['event'], 'heartbeat')
        self.assertEqual(frames[1]['id'], '7')
        self.assertEqual(json.loads(frames[1]['data']), {'entity_id': 'fixture'})
        with self.assertRaises(ValueError): p.feed(b'x' * 65537)

    def test_report_pass_fail_and_missing(self):
        budgets = json.loads((ROOT / 'deploy/scripts/perf/budgets.json').read_text())
        sample = {'started_at': 1, 'ended_at': 10, 'aborted': False, 'classes': {
            'read.board': {'count': 100, 'errors': 0, 'p95_ms': 20, 'p99_ms': 30}},
            'statuses': {'200': 100}, 'sse_latency': {'count': 1, 'p95_ms': 20}}
        obs = [{'time': 5, 'containers': [{'service': 'api', 'memory_bytes': 100},
                {'service': 'postgres', 'memory_bytes': 100}], 'oom': {'api': {'oom': 0, 'oom_kill': 0}}}]
        self.assertNotIn('FAIL', report.render(sample, obs, budgets))
        sample['statuses']['500'] = 1
        self.assertIn('FAIL', report.render(sample, obs, budgets))
        self.assertIn('NOT MEASURED', report.render({'classes': {}, 'statuses': {}}, [], budgets))

    def test_report_complete_and_phase_regression(self):
        h = common.Histogram()
        h.add(10)
        row = dict(h.as_dict(), errors=0)
        sample = {'started_at': 1, 'ended_at': 10, 'scenario': 'edit-fanout', 'target_label': 'fixture',
                  'classes': {'read.board': row, 'write.edit': row, 'heavy.recurring-edit': row},
                  'statuses': {'200': 3}, 'sse_request_to_event': h.as_dict(),
                  'stream_results': [{'received': True}]}
        obs = [{'time': 5, 'containers': [{'service': 'api', 'memory_bytes': 1}, {'service': 'postgres', 'memory_bytes': 1}],
                'oom': {'api': {'oom': 0, 'restarts': 0}, 'postgres': {'oom': 0, 'restarts': 0}}}]
        budgets = json.loads((ROOT / 'deploy/scripts/perf/budgets.json').read_text())
        self.assertIn('**Overall: PASS**', report.render(sample, obs, budgets))
        combined = report.merge_loads([sample, sample])
        self.assertEqual(combined['sse_request_to_event']['count'], 2)
        self.assertIn('**Overall: PASS**', report.render(combined, obs, budgets))
        sample['phases'] = {'ladder:50rps': {'read.board': dict(row, p95_ms=250)}}
        self.assertIn('**Overall: FAIL**', report.render(sample, obs, budgets))
        # A kernel OOM kill restarts the container and resets its cgroup counters: the restart delta must fail.
        restarted = obs + [dict(obs[0], time=6, oom={'api': {'oom': 0, 'restarts': 1}, 'postgres': {'oom': 0, 'restarts': 0}})]
        self.assertIn('| container restarts during run (OOM kills reset cgroup counters) | FAIL', report.render(combined, restarted, budgets))
        obs[0]['oom']['api']['oom'] = 1
        self.assertIn('**Overall: FAIL**', report.render(combined, obs, budgets))

    def test_observer_fixed_queries_and_units(self):
        self.assertEqual(observe.memory_bytes('1.5MiB'), 1572864)
        self.assertEqual(observe.memory_bytes('2GB'), 2000000000)
        self.assertNotIn('query,', observe.QUERIES['statements'])
        self.assertNotIn('query ', observe.QUERIES['statements'])
        calls = []
        original = observe.command
        def fake(args):
            calls.append(args)
            if args[:2] == ['docker', 'ps']: return 'abc\tapi\ndef\tpostgres'
            if args[:2] == ['docker', 'stats']: return json.dumps({'CPUPerc': '2.0%', 'MemUsage': '1MiB / 2GiB', 'PIDs': '4'})
            if args[:2] == ['docker', 'inspect']: return None
            if 'pg_stat_statements' in args[-1]: return None
            return '0'
        observe.command = fake
        try: sample = observe.sample('fixture', 'pg-fixture', 'porchlight')
        finally: observe.command = original
        self.assertEqual(len(sample['containers']), 2)
        self.assertIn('statements', sample['unavailable'])
        self.assertIn('label=com.docker.compose.project=fixture', calls[0])
        self.assertTrue(all(a[3] == '{{.State.Pid}} {{.RestartCount}}' for a in calls if a[:2] == ['docker', 'inspect']))

    def test_old_history_completion_supplies_version(self):
        plan = {'tier': 'mature', 'steps': [], 'prefix': 'fixture', 'anchor': '2026-11-13', 'counts': {}}
        path = self.out / 'journal.jsonl'
        seed.append(path, {'plan_hash': common.digest(plan)})
        for key in ['history', 'historylist', 'historyentry0', 'member0']:
            seed.append(path, {'state': 'done', 'key': key, 'result': {'id': key}})
        calls = []
        def porch(args):
            calls.append(args)
            if args[:2] == ['occurrences', 'list']:
                return {'items': [{'id': 'historic', 'status': 'todo', 'version': 7}]}
            if args[:2] == ['occurrences', 'complete']:
                self.assertIn('--version', args, 'historic occurrence is outside CLI snapshot; explicit version required')
                self.assertEqual(args[args.index('--version')+1], '7')
            return {}
        seed.apply(plan, self.out, porch)
        self.assertEqual(sum(a[:2] == ['occurrences', 'complete'] for a in calls), 1)

    def test_completion_uses_authorized_item(self):
        class Rng:
            def choice(self, rows): return rows[0]
        d = load.Device.__new__(load.Device)
        d.rng = Rng()
        d.items = [{'id': 'read-only'}, {'id': 'owned'}]
        d.completable = [d.items[1]]
        calls = []
        def request(cls, method, path, *args):
            calls.append(path)
            return {'items': [{'id': 'occurrence', 'version': 1, 'status': 'todo'}]}
        d.request = request
        d.operation(76)
        self.assertEqual(calls[0], '/v1/items/owned/occurrences?undated=true&limit=200')

    def test_keepalive_headers_and_stats(self):
        class Reply:
            status = 200
            def __init__(self): self.sent = False
            def read(self, size):
                if self.sent: return b''
                self.sent = True
                return b'{"version":2}'
        class Conn:
            def __init__(self): self.requests = []
            def request(self, *args): self.requests.append(args)
            def getresponse(self): return Reply()
            def close(self): pass
        class Limiter:
            deadline = load.time.monotonic()+10
            def acquire(self): pass
        d = load.Device.__new__(load.Device)
        d.conn = Conn()
        d.token, d.version, d.limiter = 'synthetic-fixture', '0.1.7', Limiter()
        d.metrics = load.Metrics(3)
        self.assertEqual(d.request('write.edit', 'PATCH', '/v1/items/fixture', {'notes': 'fixture'}, 1), {'version': 2})
        d.request('read.item', 'GET', '/v1/items/fixture')
        self.assertEqual(len(d.conn.requests), 2)
        h = d.conn.requests[0][3]
        self.assertEqual(h['If-Match'], '1')
        self.assertTrue(h['Idempotency-Key'])
        self.assertEqual(h['Porchlight-Client-Version'], '0.1.7')
        self.assertNotIn('Idempotency-Key', d.conn.requests[1][3])
        for _ in range(3): d.metrics.record('read.item', 500, 1)
        self.assertTrue(d.metrics.stop.is_set())

    def test_fake_cli_apply_verify_resume(self):
        fake = self.out / 'porch'
        fake.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
p=pathlib.Path(os.environ['FAKE_STATE'])
s=json.loads(p.read_text()) if p.exists() else {}
a=sys.argv[1:]; a=a[a.index('--json')+1:]
k=' '.join(a[:2])
if a[:2]==['users','list']: r={'items':s.get('members',[])}
elif a[:2]==['items','list']: r={'items':s.get('items',[])}
elif a[:2]==['lists','list']: r={'items':s.get('lists',[]),'next_cursor':None}
elif a[:2]==['users','add']:
 r={'id':'member-'+str(len(s.get('members',[]))), 'username':a[a.index('--username')+1]}; s.setdefault('members',[]).append(r)
elif a[:2]==['items','add']:
 r={'id':'item-'+str(len(s.get('items',[]))), 'title':a[a.index('--title')+1]}; s.setdefault('items',[]).append(r)
elif a[:2]==['lists','add']:
 r={'id':'list-'+str(len(s.get('lists',[]))), 'name':a[2]}; s.setdefault('lists',[]).append(r)
elif a[:2]==['lists','show']: r={'entries':[e for e in s.get('entries',[]) if not e.get('checked_at')], 'got_it':[e for e in s.get('entries',[]) if e.get('checked_at')]}
elif a[:2]==['lists','add-entry']:
 r={'id':'entry-'+str(len(s.get('entries',[]))), 'label':a[3]}; s.setdefault('entries',[]).append(r)
elif a[:2]==['comments','list']: r={'items':s.get('comments',[]),'next_cursor':None}
elif a[:2]==['comments','add']:
 r={'id':'comment-'+str(len(s.get('comments',[]))), 'body':a[3]}; s.setdefault('comments',[]).append(r)
elif a[:2]==['occurrences','list']: r={'items':[]}
elif a[0]=='attach': r={'id':'attachment', 'original_name':pathlib.Path(a[2]).name}; s.setdefault('attachments',[]).append(r)
elif a[0]=='export':
 out=pathlib.Path(a[a.index('--out')+1]); out.write_text(json.dumps({'tables':{'attachments':s.get('attachments',[])}})); out.chmod(0o600); r={}
else: sys.exit(3)
p.write_text(json.dumps(s)); print(json.dumps(r))
''')
        fake.chmod(0o700)
        token = self.out / 'token'
        common.write_private(token, 'fixture')
        env = dict(os.environ, PATH=str(self.out)+os.pathsep+os.environ['PATH'], FAKE_STATE=str(self.out / 'state'))
        args = [sys.executable, '-I', str(ROOT / 'deploy/scripts/perf/seed-household.py'),
                '--out', str(self.out / 'seed'), '--socket', str(self.out / 'agent.sock'),
                '--token-file', str(token), '--scale', '0.001', '--anchor', '2026-11-13']
        for cmd in ['apply', 'verify', 'apply', 'verify']:
            r = subprocess.run(args+[cmd], env=env, capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stdout+r.stderr)
        state = json.loads((self.out / 'state').read_text())
        self.assertEqual(len(state['items']), 2)
        self.assertEqual(len(state['members']), 6)
        self.assertEqual((self.out / 'seed/journal.jsonl').stat().st_mode & 0o777, 0o600)
        self.assertEqual(len(state['attachments']), 1)
        state['entries'][0]['checked_at'] = 'synthetic-fixture'
        (self.out / 'state').write_text(json.dumps(state))
        r = subprocess.run(args+['verify'], env=env, capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, 'checked entries in got_it must verify: '+r.stderr)
        # Crash after attachment commit, before its done line: export label reconciliation must recover it.
        journal = self.out / 'seed/journal.jsonl'
        rows = [json.loads(line) for line in journal.read_text().splitlines()]
        rows = [r for r in rows if not (r.get('key') == 'attachment0' and r.get('state') == 'done')]
        common.write_private(journal, ''.join(json.dumps(r)+'\n' for r in rows))
        r = subprocess.run(args+['apply'], env=env, capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(len(json.loads((self.out / 'state').read_text())['attachments']), 1)
        # Missing/loose credentials fail without invoking the CLI or creating seed state.
        token.chmod(0o644)
        r = subprocess.run(args+['apply'], env=env, capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)
        token.unlink()
        r = subprocess.run(args+['apply'], env=env, capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)

def load_tests(loader, tests, pattern):
    spec = importlib.util.spec_from_file_location('perf_harness2', Path(__file__).with_name('perf_harness2.py'))
    extra = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(extra)
    tests.addTests(loader.loadTestsFromModule(extra))
    return tests


if __name__ == '__main__': unittest.main()
