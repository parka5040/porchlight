#!/usr/bin/env python3
"""S7b: compare documentation with columns parsed from the actual unreleased migrations.
Parses CREATE TABLE only; ALTER TABLE ADD COLUMN is not recognized. When adding columns
via ALTER TABLE, extend this parser or verify their documentation explicitly in review.
"""
from pathlib import Path
import re

root = Path(__file__).resolve().parent.parent
matrix = (root / "docs/specs/derivation-matrix.md").read_text()
locks = (root / "docs/specs/lock-order.md").read_text()

def fields(sql):
    """Split at top-level commas, respecting nested constraints and quoted SQL strings."""
    start = depth = 0
    quoted = False
    for i, char in enumerate(sql):
        if char == "'":
            quoted = not quoted
        elif not quoted:
            if char == "(": depth += 1
            elif char == ")": depth -= 1
            elif char == "," and depth == 0:
                yield sql[start:i].strip()
                start = i + 1
    yield sql[start:].strip()

schema = {}
for migration in sorted((root / "server/db/migrations").glob("003[5-9]_*.sql")):
    sql = re.sub(r"--[^\n]*", "", migration.read_text())
    for match in re.finditer(r"CREATE TABLE\s+(\w+)\s*\(", sql, re.I):
        tail = sql[match.end():]
        depth, quoted = 1, False
        for end, char in enumerate(tail):
            if char == "'": quoted = not quoted
            elif not quoted:
                if char == "(": depth += 1
                elif char == ")": depth -= 1
                if depth == 0: break
        else: raise AssertionError("Unbalanced CREATE TABLE")
        schema[match[1]] = [field.split()[0] for field in fields(tail[:end])
                            if not re.match(r"(?:PRIMARY|FOREIGN|UNIQUE|CHECK|CONSTRAINT)\b", field, re.I)]
assert schema, "No collaboration tables parsed"
for table, columns in schema.items():
    rows = [line for line in matrix.splitlines() if line.startswith("|") and f"`{table}`" in line]
    assert rows, f"Undocumented table: {table}"
    for column in columns:
        assert any(f"`{column}`" in row for row in rows), f"Undocumented column: {table}.{column}"
for name in ["comment.changed", "attachment.changed", "pl_take_blob_lease", "emit_comment_notifications_s4b",
             "snapshot_comments_s4b", "snapshot_attachments_s4b", "comment_search_source()"]:
    assert name in matrix, f"Undocumented collaboration contract: {name}"
for contract in ["S7b collaboration and blob locks", "UNION", "ascending", "GC", "pl_take_blob_lease", "one household per data volume"]:
    assert contract in locks, f"Undocumented collaboration lock rule: {contract}"
assert "`search_document` (0037)" in matrix, "Undocumented generated comment search column"
reads = (root / "server/api/src/routes/reads.rs").read_text()
for hook in ["snapshot_comments_s4b", "snapshot_attachments_s4b"]:
    assert hook in reads, f"Unwired snapshot hook: {hook}"
source = (root / "server/api/src/collaboration.rs").read_text()
assert "UNION ALL {comments}" in reads and "comment_search_source()" in reads, "Unwired comment FTS source"
assert "pub fn comment_search_source() -> String" in source, "Missing ACL-complete comment FTS function"
assert "c.search_document" in source and "WHERE {VISIBLE}" in source, "Missing shared comment FTS/ACL source"
for path in ["server/api/src/collaboration.rs", "server/api/src/blobs.rs"]:
    assert ("TODO" + "(S4b)") not in (root / path).read_text(), f"Deferred integration marker: {path}"
assert "pl_emit_notification_event" in (root / "server/api/src/collaboration.rs").read_text()
print(f"S7b collaboration migration schema, integrated hooks and blob lock contract are current ({len(schema)} tables)")
