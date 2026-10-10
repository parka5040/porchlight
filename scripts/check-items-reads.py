#!/usr/bin/env python3
"""S4b schema/producer contract documentation gate; DB invariants check actual grants."""
from pathlib import Path
import re
root = Path(__file__).resolve().parent.parent
matrix = (root / 'docs/specs/derivation-matrix.md').read_text()
contract = (root / 'docs/specs/items-reads.md').read_text()
for name in ['search_document', 'notification_events', 'notification_kinds', 'processed_at', 'member_settings', 'settings.changed']:
    assert f'`{name}`' in matrix, f'missing derivation bookkeeping: {name}'
for name in ['pl_item_can_read', 'pl_emit_notification_event', 'today_sections', 'this_week_days', 'CET', 'REPEATABLE READ']:
    assert name in contract, f'missing read contract: {name}'
# s8a/R53: later migrations extend kinds as data, never replace the producer ABI.
for name in ['notification_kinds', 'self_suppressed', 'calendar_changed', 'feed_error', 'ON CONFLICT (kind) DO NOTHING']:
    assert name in contract, f'missing kind-registry contract: {name}'
abi_definitions = []
for migration in sorted((root / 'server/db/migrations').glob('*.sql')):
    sql = re.sub(r'--[^\n]*', '', migration.read_text())
    if re.search(r'CREATE\s+(?:OR\s+REPLACE\s+)?FUNCTION\s+(?:porchlight\.)?pl_emit_notification_event\b', sql, re.I):
        abi_definitions.append(migration.name)
    assert 'notification_events_kind_check' not in sql, f'closed kind CHECK in {migration.name}'
assert abi_definitions == ['0015_items_reads.sql'], f'producer ABI redefined: {abi_definitions}'
print('Items read, settings and notification-event contracts are documented')
print('R53 notification kinds extend one registry and one producer ABI')
