#!/usr/bin/env python3
"""Keep S3's additive authoritative/schema bookkeeping changes in the dependency matrix."""
from pathlib import Path
import re

root = Path(__file__).resolve().parent.parent
migration = (root / "server/db/migrations/0012_items_derivation.sql").read_text()
matrix = (root / "docs/specs/derivation-matrix.md").read_text()
lock_order = (root / "docs/specs/lock-order.md").read_text()
columns = re.findall(r"ADD COLUMN (\w+)", migration)
missing = [name for name in columns if f"`{name}`" not in matrix]
for name in ["scheduler_jobs", "last_run_at", "last_success_at", "next_due_at", "last_error", "attempts"]:
    if f"`{name}`" not in matrix:
        missing.append(name)
assert not missing, f"Derivation matrix missing schema entries: {missing}"
assert "{item_id, applied_gen}" in matrix and "error: true" in matrix
for contract in ["S3 conflict/write paths", "Worker:", "Time reports:", "Scheduler:", "FOR NO KEY UPDATE", "SKIP LOCKED"]:
    assert contract in lock_order, f"Lock-order document missing {contract}"
print(f"Derivation matrix and lock contract are current ({len(columns)} authoritative additions)")
