#!/usr/bin/env python3
"""Keep the administrative security-history data-out and privacy exemptions explicit."""
from pathlib import Path
import re

root = Path(__file__).resolve().parents[1]
read = lambda path: (root / path).read_text()
sql = read('server/db/migrations/0070_security_events.sql')
spec = read('docs/specs/security-events.md')
exports = read('server/api/src/routes/exports.rs')
allowlist = exports.split('const HOUSEHOLD_TABLES:', 1)[1].split('];', 1)[0]
assert '"security_events"' in allowlist
for kind in re.findall(r"'([a-z_]+)'", sql.split('kind text NOT NULL CHECK (kind IN (', 1)[1].split(')),', 1)[0]):
    assert f'`{kind}`' in spec, kind
for fragment in [
    'DEFAULT pl_current_actor()',
    'BEFORE UPDATE OR DELETE OR TRUNCATE ON security_events',
    'GRANT INSERT(household_id,kind,target_type,target_id,outcome)',
    'GRANT SELECT,DELETE ON security_events TO porchlight_maintenance',
    "(10,'security_events','occurred_at<$2-interval ''1 year''',true)",
]:
    assert fragment in sql, fragment
assert not re.search(r'GRANT[^;]+security_events[^;]+porchlight_agent', sql)
for doc in ['derivation-matrix.md', 'data-out.md', 'lock-order.md']:
    assert 'security' in read('docs/specs/' + doc).lower()
assert 'security_events' in read('docs/operations/backup-restore.md')
assert 'security_events' in read('server/api/tests/trust/audit_log.rs')
assert (root / 'server/cli/tests/goldens/security-events.json').is_file()
assert 'Server-only scope is owner-approved' in read('docs/specs/derivation-matrix.md')
print('Security history grants, retention, export inventory and server-only exemption are current')
