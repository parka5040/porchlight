#!/usr/bin/env python3
"""Live M3 checks. All command bodies/diagnostics stay in the private gate directory."""
import datetime as dt
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import traceback
import uuid


class Gate:
    def __init__(self, root, base, porch):
        self.root, self.base, self.porch = Path(root), Path(base), porch
        self.dc_args = ['docker', 'compose', '-p', 'porchlight-m3demo', '-f',
                        str(self.root / 'deploy/compose.dev.yml'), '--env-file',
                        str(self.base / 'compose.env')]
        self.log = open(self.base / 'driver.log', 'a', encoding='utf-8')

    def execute(self, args, data=None, expected=0):
        result = subprocess.run(args, input=data, text=True, capture_output=True, timeout=90)
        self.log.write(result.stdout + result.stderr)
        self.log.flush()
        if result.returncode != expected:
            raise AssertionError('operation exit code (see private log)')
        return result.stdout

    def dc(self, *args):
        return self.execute(self.dc_args + list(args))

    def cli(self, *args, expected=0):
        return json.loads(self.execute([self.porch, '--socket', str(self.base / 'run/agent.sock'),
                                       '--token-file', str(self.base / 'bootstrap.pat'), '--json',
                                       *map(str, args)], expected=expected))

    def sql(self, query, agent=False):
        # Password stays in the container environment, not host argv. Domain
        # writes use the REAL login agent role, never SET ROLE from a superuser.
        if agent:
            command = ['sh', '-c', 'export PGPASSWORD="$PORCHLIGHT_AGENT_PASSWORD"; '
                       'export PGAPPNAME=agent:demo; '
                       'exec psql -X -h 127.0.0.1 -U porchlight_agent -d porchlight -At -v ON_ERROR_STOP=1']
        else:
            command = ['psql', '-X', '-U', 'postgres', '-d', 'porchlight', '-At', '-v', 'ON_ERROR_STOP=1']
        return self.execute(self.dc_args + ['exec', '-T', 'postgres'] + command, query + '\n').strip()

    @staticmethod
    def check(name, condition):
        if not condition:
            raise AssertionError(name)
        print('PASS: M3 ' + name, flush=True)

    def wait(self, query, expected='t'):
        end = time.monotonic() + 30
        while time.monotonic() < end:
            if self.sql(query) == expected:
                return
            time.sleep(0.2)
        raise AssertionError('worker convergence deadline')

    def occurrences(self, item):
        return self.cli('occurrences', 'list', item)['items']

    def run(self):
        today = dt.datetime.now(dt.timezone.utc).replace(hour=12, minute=0, second=0, microsecond=0)
        future = today + dt.timedelta(days=30)
        iso = lambda value: value.isoformat().replace('+00:00', 'Z')
        init = json.load(open(self.base / 'init.json'))
        who = self.cli('whoami')
        member, household = who['member_id'], init['household_id']
        calendar = self.cli('items', 'add', '--title', 'Calendar', '--kind', 'deadline',
                            '--due', iso(future), '--tz', 'UTC', '--rrule', 'FREQ=DAILY;COUNT=4')
        item = calendar['id']
        occs = self.occurrences(item)
        self.check('calendar recurrence', len(occs) == 4)
        completion = self.cli('items', 'add', '--title', 'Completion', '--kind', 'deadline',
                              '--due', iso(future), '--tz', 'UTC', '--every', '2d')
        closed = self.cli('occurrences', 'complete', self.occurrences(completion['id'])[0]['id'])
        successors = self.occurrences(completion['id'])
        self.check('completion recurrence has exactly one successor', closed['status'] == 'done' and
                   sum(o['status'] not in ('done', 'skipped') for o in successors) == 1)
        self.cli('items', 'edit', item, '--title', 'Calendar all')
        detached = self.cli('occurrences', 'override', occs[0]['id'], '--due',
                            (future + dt.timedelta(hours=1)).replace(tzinfo=None).isoformat())
        self.check('this occurrence scope', detached['is_detached'])
        tail = self.cli('items', 'edit', item, '--scope', 'this-and-future', '--occurrence',
                        occs[2]['id'], '--title', 'Tail')
        self.check('this-and-future scope creates lineage', tail['id'] != item and
                   tail['split_from_item_id'] == item)
        # Missed fixtures are relative to this measured UTC day with whole-day
        # margins, not fixed historical dates or a production clock hook. Future
        # fixtures above remain inside the rolling horizon on every gate run.
        for policy, expected in [('carry_over', ['expired', 'expired', 'overdue']),
                                 ('stack', ['overdue'] * 3), ('skip', ['expired'] * 3)]:
            past = today - dt.timedelta(days=4)
            fixture = self.cli('items', 'add', '--title', 'Missed ' + policy, '--kind', 'deadline',
                               '--due', iso(past), '--tz', 'UTC', '--rrule', 'FREQ=DAILY;COUNT=3',
                               '--missed', policy)
            rows = sorted(self.occurrences(fixture['id']), key=lambda r: r['seq'])
            self.check('missed policy ' + policy, [r['disposition'] for r in rows] == expected)
        original = self.cli('items', 'add', '--title', 'Revert original', '--assign', 'admin')
        rev = self.cli('revisions', original['id'])['items'][-1]['id']
        changed = self.cli('items', 'edit', original['id'], '--title', 'Revert changed')
        stale = self.cli('items', 'edit', original['id'], '--title', 'Stale', '--version',
                         original['version'], expected=1)
        self.check('stale writes return 409 current', stale['status'] == 409 and
                   stale['current']['version'] == changed['version'])
        restored = self.cli('revert', original['id'], '--to', rev)
        self.check('revert creates an attributed revision', restored['title'] == original['title'] and
                   restored['version'] > changed['version'] and not restored['pending_derivation'])
        audit = self.sql("select actor->>'application_name' from porchlight.item_revisions "
                         f"where item_id='{original['id']}' order by rev desc limit 1")
        self.check('agent revision attribution', audit.startswith('agent:'))
        # Stop only this isolated API to eliminate the worker race. The future
        # retry timestamp is operational test setup, never a domain agent write.
        self.dc('stop', 'api')
        raw = str(uuid.uuid4())
        self.sql("insert into porchlight.items(id,household_id,owner_id,title,kind,tzid,starts_at,ends_at) "
                 f"values('{raw}','{household}','{member}','Raw pending','appointment','UTC',"
                 f"'{iso(future)}','{iso(future + dt.timedelta(hours=1))}');", agent=True)
        self.sql(f"update porchlight.item_derivation set retry_at='2099-01-01' where item_id='{raw}'")
        self.dc('up', '-d', '--no-deps', '--wait', 'api')
        pending = self.cli('items', 'show', raw)
        self.check('raw SQL exposes pending flag', pending['pending_derivation'])
        overlap = self.cli('items', 'add', '--title', 'Overlap', '--start', iso(future),
                           '--end', iso(future + dt.timedelta(hours=1)), '--tz', 'UTC')
        self.check('API drains pending raw SQL before hard-conflict check',
                   any(c['other_item_id'] == raw and c['kind'] == 'hard' for c in overlap['conflicts']) and
                   not self.cli('items', 'show', raw)['pending_derivation'])
        again = self.cli('items', 'add', '--title', 'Hard conflict', '--start', iso(future),
                         '--end', iso(future + dt.timedelta(hours=1)), '--tz', 'UTC')
        self.check('ordinary hard conflicts', any(c['kind'] == 'hard' for c in again['conflicts']))
        raw_occ = self.occurrences(raw)[0]['id']
        self.sql(f"update porchlight.item_occurrences set status='done',completed_by='{member}',"
                 f"completed_at=now(),board_position='8080' where id='{raw_occ}';", agent=True)
        self.cli('derive', 'retry', raw)
        self.wait(f"select applied_gen=requested_gen from porchlight.item_derivation where item_id='{raw}'")
        row = self.occurrences(raw)[0]
        self.check('raw SQL complete and board move converge', row['status'] == 'done' and
                   row['board_position'] == '8080' and not row['pending_derivation'])
        poison = str(uuid.uuid4())
        self.sql("insert into porchlight.items(id,household_id,owner_id,title,kind,due_date,rrule,recurrence_mode) "
                 f"values('{poison}','{household}','{member}','Poison','todo','{future.date()}',"
                 "'FREQ=DAILY;COUNT=2;COUNT=3','calendar');", agent=True)
        self.wait(f"select last_error is not null from porchlight.item_derivation where item_id='{poison}'")
        status = self.cli('derive', 'status')
        self.check('worker poison records attempts and last_error', any(r['item_id'] == poison and
                   r['attempts'] >= 1 and r['last_error'] for r in status['items']))
        # Retain terminal retry state across repair, then explicitly release it.
        self.sql(f"update porchlight.item_derivation set attempts=5 where item_id='{poison}'")
        self.sql(f"update porchlight.items set rrule='FREQ=DAILY;COUNT=2' where id='{poison}';", agent=True)
        self.cli('derive', 'retry', poison)
        self.wait(f"select applied_gen=requested_gen and last_error is null from porchlight.item_derivation where item_id='{poison}'")
        self.check('derive retry repairs worker failure', len(self.occurrences(poison)) == 2)
        live = self.cli('purge', '--item', original['id'], '--yes', expected=1)
        self.check('purge refuses live items', live['status'] == 422)
        self.cli('items', 'rm', original['id'])
        purged = self.cli('purge', '--item', original['id'], '--yes')
        count = self.sql(f"select count(*) from porchlight.purge_log where id='{purged['id']}' "
                         "and actor->>'application_name' like 'agent:%' and counts->>'items'='1'")
        self.check('purge has durable actor and counts audit', count == '1' and
                   self.sql(f"select count(*) from porchlight.items where id='{original['id']}'") == '0')
        self.check('CLI reads use shared projections', bool(self.cli('board')['items']) and
                   bool(self.cli('search', 'Calendar')['items']))


def main():
    gate = Gate(*sys.argv[1:])
    try:
        gate.run()
    except Exception:
        traceback.print_exc(file=gate.log)
        gate.log.flush()
        print('FAIL: M3 live checks (private diagnostics under var/m3-demo)', file=sys.stderr)
        return 1
    finally:
        gate.log.close()
    return 0


if __name__ == '__main__':
    sys.exit(main())
