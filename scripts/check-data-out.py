#!/usr/bin/env python3
"""Cheap O2 server contract guard; behavioral regressions run in workspace tests."""
from pathlib import Path
import re

root = Path(__file__).resolve().parents[1]
read = lambda path: (root / path).read_text()
stale = read('server/db/migrations/0057_backup_stale.sql')
assert re.search(r'INSERT INTO porchlight\.notification_kinds\(kind,self_suppressed,description\)', stale)
assert "VALUES('backup_stale',false," in stale
assert 'ON CONFLICT (kind) DO NOTHING' in stale
assert not re.search(r'CREATE\s+(OR\s+REPLACE\s+)?FUNCTION|ALTER\s+TABLE', stale, re.I), 'R53: registry data only'
assert '| backup_stale | no | 0057 | No successful household backup for more than 26 hours |' in read('docs/specs/items-reads.md'), 'R53 canonical registry row'
assert 'later-fold' in read('docs/specs/data-out.md'), 'K7 later-fold export limitation'
retention = read('server/db/migrations/0056_retention_jobs.sql')
for fragment in ["interval '90 days'", "interval '7 days'", 'stream_assign_mutex', 'epoch=epoch+1', 'expires_at>clock_timestamp()', 'FOR SHARE OF b']:
    assert fragment in retention, fragment
exports = read('server/api/src/routes/exports.rs')
allowlist = exports.split('const HOUSEHOLD_TABLES:', 1)[1].split('];', 1)[0]
for secret in ['user_credentials', 'sessions', 'enrollment_tokens', 'password_resets', 'api_tokens', 'step_ups', 'idempotency_keys', 'ics_feed_tokens', 'install_state']:
    assert f'"{secret}"' not in allowlist, secret
for name in ['scheduled_reminders', 'comments', 'attachments', 'notification_event_delivery', 'security_events']:
    assert f'"{name}"' in allowlist, name
for name in ['stream_retention', 'pl_renew_blob_lease', 'backup_stale', 'purge_log']:
    assert f'`{name}`' in read('docs/specs/derivation-matrix.md'), name
for path, prefix in [('deploy/compose.yml', '${PORCHLIGHT_ROOT:-/var/lib/porchlight}'), ('deploy/compose.dev.yml', '${PORCHLIGHT_VAR:-../var}')]:
    assert f'{prefix}/backups:/backups:ro' in read(path), 'API backup mount must be read-only'
assert 'publication_digest(encrypted, lease)' in read('deploy/backup/backup.py')
print('O2 server export, retention, notification registry and backup lease contracts are current')
