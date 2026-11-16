#!/usr/bin/env python3
"""Keep negotiated stream fields, invalidations and their bounded contracts current."""
from pathlib import Path
import json

root = Path(__file__).resolve().parent.parent
stream = (root / "docs/specs/stream.md").read_text()
matrix = (root / "docs/specs/derivation-matrix.md").read_text()
locks = (root / "docs/specs/lock-order.md").read_text()
notifications = (root / "docs/specs/notifications.md").read_text()
permissions = (root / "docs/specs/permissions-matrix.md").read_text()
migration = (root / "server/db/migrations/0078_targeted_stream.sql").read_text()
api = json.loads((root / "docs/openapi.json").read_text())
params = api["paths"]["/v1/stream"]["get"]["parameters"]
assert any(p["name"] == "Porchlight-Capabilities" and p["in"] == "header" for p in params)
schema = api["components"]["schemas"]["StreamTickle"]
for field in ["item_id", "list_id"]:
    assert field in schema["properties"] and field not in schema.get("required", [])
for kind in ["item.invalidated", "notification.invalidated", "settings.changed", "sync.reset"]:
    assert kind in migration and kind in stream and kind in matrix, kind
assert "notification.invalidated" in notifications
for case in ["item.invalidated", "notification.invalidated", "resnapshot/projection_changed"]:
    assert case in permissions, case
assert "pl_sync_emit" in locks and "200" in stream and "LIMIT 201" in migration
assert "FOR UPDATE" not in migration and "FOR NO KEY UPDATE" not in migration
assert "pl_enqueue_derivation" not in migration
integrity = (root / "server/db/migrations/0079_targeted_stream_integrity.sql").read_text()
assert "current_setting" not in integrity and "set_config" not in integrity
for contract in ["sync_emissions", "pg_current_xact_id()", "FOR UPDATE SKIP LOCKED", "LIMIT 200",
                 "REVOKE ALL ON sync_emissions", "pl_sync_actor_rename", "CREATE TRIGGER sync_pending"]:
    assert contract in integrity, contract
for contract in ["sync_emissions", "0079", "pending", "actor"]:
    assert contract in matrix and contract in stream, contract
assert "sync_emissions" in locks
proof = root / "server/api/tests/live/prerollout_sync_server_b/coverage/proof"
assert (proof / "mutations.rs").is_file() and (proof / "writers.rs").is_file()
route = (root / "server/api/src/routes/stream.rs").read_text()
catch_up_tests = (root / "server/api/tests/live/prerollout_sync_server_d.rs").read_text()
assert "caught_up" in api["paths"]["/v1/stream"]["get"]["summary"]
for contract in ["caught_up", "connect watermark", "no SSE id", "Bounded background catch-up recipe"]:
    assert contract in stream, contract
assert "live.item_sync.then_some(window.maximum)" in route
assert 'self.catch_up_watermark.take()' in route
for case in ["empty_backlog_is_ready_then_caught_up_once",
             "filtered_backlog_and_concurrent_publish_preserve_connect_watermark",
             "wholly_filtered_backlog_still_catches_up",
             "global_watermark_covers_other_households_without_local_events",
             "slow_replay_allows_heartbeat_but_finishes_backlog_first",
             "drain_and_revocation_win_over_pending_caught_up",
             "legacy_stream_bytes_are_unchanged", "invalid_and_expired_cursors_only_resnapshot",
             "revocation_during_replay_wins_over_caught_up"]:
    assert f"async fn {case}(" in catch_up_tests, case
print("Targeted stream negotiation, catch-up, parent hints, bounded invalidations and lock contracts are current")
