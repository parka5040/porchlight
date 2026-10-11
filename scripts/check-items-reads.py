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
def check_migrations(migrations):
    abi_definitions = []
    for name, source in migrations:
        # A cheap lexical guard, complemented by the final migrated catalog tests.
        sql = re.sub(r'/\*.*?\*/|--[^\n]*', ' ', source, flags=re.S)
        sql = re.sub(r'"([a-z_][a-z_0-9]*)"', r'\1', sql, flags=re.I)
        sql = re.sub(r'\s*\.\s*', '.', sql)
        if re.search(r'CREATE\s+(?:OR\s+REPLACE\s+)?FUNCTION\s+(?:porchlight\.)?pl_emit_notification_event\b', sql, re.I):
            abi_definitions.append(name)
        assert not re.search(r'\bUPDATE\s+(?:ONLY\s+)?(?:porchlight\.)?notification_kinds\b', sql, re.I), f'registry UPDATE in {name}'
        assert 'notification_events_kind_check' not in sql, f'closed kind CHECK in {name}'
        for statement in sql.split(';'):
            if not re.search(r'\b(?:ALTER|CREATE)\s+TABLE\s+(?:ONLY\s+)?(?:porchlight\.)?notification_events\b', statement, re.I):
                continue
            # Ignore literal contents and balance nested CHECK parentheses; names
            # and formatting must not let another closed kind list evade the guard.
            statement = re.sub(r"'(?:[^']|'')*'", "''", statement)
            for check in re.finditer(r'\bCHECK\s*\(', statement, re.I):
                depth, end = 1, check.end()
                while end < len(statement) and depth:
                    depth += (statement[end] == '(') - (statement[end] == ')')
                    end += 1
                assert not re.search(r'\bkind\b', statement[check.end():end], re.I), f'closed kind CHECK in {name}'
    assert abi_definitions == ['0015_items_reads.sql'], f'producer ABI redefined: {abi_definitions}'


check_migrations((path.name, path.read_text()) for path in sorted((root / 'server/db/migrations').glob('*.sql')))
print('Items read, settings and notification-event contracts are documented')
print('R53 notification kinds extend one registry and one producer ABI')
