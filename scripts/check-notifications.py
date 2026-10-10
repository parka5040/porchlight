#!/usr/bin/env python3
"""S6a docs and additive integration freshness guard; behavior/grants are tested in Rust."""
import ast
from pathlib import Path
root = Path(__file__).resolve().parent.parent
contract = (root / 'docs/specs/notifications.md').read_text()
matrix = (root / 'docs/specs/derivation-matrix.md').read_text()
locks = (root / 'docs/specs/lock-order.md').read_text()
for name in ['notifications', 'notification_sources', 'notification_prefs', 'default_appointment_offsets', 'processed_at']:
    assert f'`{name}`' in matrix, f'missing derivation entry: {name}'
for name in ['notification.created', 'notification.updated', 'notification.dismissed', 'settings.changed', '30-second', 'Sunday 18:00', 'REPEATABLE READ', 'pl_emit_notification_event']:
    assert name in contract, f'missing notification contract: {name}'
assert 'notification_prefs' in locks and 'SKIP LOCKED' in locks
ast.parse((root / 'deploy/scripts/m5-notifications-demo.py').read_text())
print('Notification schema, routing, source ACLs, scheduler and lock contracts are documented')
