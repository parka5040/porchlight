#!/usr/bin/env python3
"""Mutation probes for the cheap R53 migration guard (catalog tests run in Rust)."""
from pathlib import Path
import runpy

root = Path(__file__).resolve().parents[2]
check = runpy.run_path(str(root / 'scripts/check-items-reads.py'))['check_migrations']
base = [('0015_items_reads.sql', 'CREATE FUNCTION pl_emit_notification_event() RETURNS uuid AS $$ BEGIN RETURN NULL; END $$ LANGUAGE plpgsql;')]
mutations = [
    'CREATE OR REPLACE FUNCTION "porchlight" . "pl_emit_notification_event"() RETURNS uuid AS $$ BEGIN RETURN NULL; END $$ LANGUAGE plpgsql;',
    'CREATE /* quoted ABI */ FUNCTION "pl_emit_notification_event"() RETURNS uuid AS $$ BEGIN RETURN NULL; END $$ LANGUAGE plpgsql;',
    'ALTER TABLE "porchlight"."notification_events" ADD CONSTRAINT "different_name" CHECK (kind IN (\'assigned\'));',
    'ALTER TABLE notification_events ADD CHECK ((length(entity_type)>0) AND "kind" IN (\'assigned\'));',
    'UPDATE "porchlight" . "notification_kinds" SET self_suppressed=false;',
    'UPDATE ONLY notification_kinds SET description=\'changed\';',
    "CREATE TABLE notification_events (kind text CHECK (kind='assigned'));",
]
for mutation in mutations:
    try:
        check(base + [('9999_mutation.sql', mutation)])
    except AssertionError:
        pass
    else:
        raise AssertionError(f'migration guard accepted mutation: {mutation}')
check(base + [('9999_extension.sql', "INSERT INTO notification_kinds VALUES ('future_kind',false,'Future') ON CONFLICT(kind) DO NOTHING;")])
check(base + [('9999_unrelated.sql', "ALTER TABLE porchlight.notification_events ADD CHECK (entity_type <> 'kind'); -- CREATE FUNCTION pl_emit_notification_event()")])
print(f'PASS R53 migration guard rejects {len(mutations)} bypass mutations and accepts registry extension')
