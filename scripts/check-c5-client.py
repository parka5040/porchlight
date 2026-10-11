#!/usr/bin/env python3
"""C5 offline contract boundary checks; Kotlin suites test repository/UI behaviour."""
import json
import re
from pathlib import Path
import xml.etree.ElementTree as ET

root = Path(__file__).resolve().parent.parent
fixtures = root / "client/shared/src/jvmTest/resources/item-fixtures"
snapshot = json.loads((fixtures / "snapshot.json").read_text())
for name, section in (("list", "lists"), ("comment", "comments"), ("attachment", "attachments")):
    row = json.loads((fixtures / (name + ".json")).read_text())
    assert row in snapshot[section], f"{name} recording is not the same real snapshot metadata"
for name in ("list", "list-attached-persistent", "list-occurrence-template", "list-occurrence-check", "list-occurrence-uncheck", "list-occurrence-other", "list-occurrence-assignee", "list-persistent-assignee"):
    row = json.loads((fixtures / (name + ".json")).read_text())
    for entry in row["entries"] + row["got_it"]:
        assert (entry["checked_at"] is None) == (entry["checked_by"] is None), f"{name}: corrupt check state"
        assert entry["occurrence_id"] == row["occurrence_id"], f"{name}: mixed scope"
    if row["mode"] == "per_occurrence":
        assert not row["got_it"], "Per-occurrence checks stay in entries"
sql = (root / "client/shared/src/commonMain/sqldelight/app/porchlight/shared/db/Cache.sq").read_text()
promotions = re.search(r"\bpromoteEntities\s*\{([^}]*)\}", sql, re.S)
assert promotions, "Snapshot metadata must be promoted in one SQLDelight transaction group"
for name in ("List", "Entry", "Comment", "Attachment"):
    assert f"CREATE TABLE Cache{name}" in sql
    assert re.search(rf"INSERT INTO Cache{name}\s+SELECT\b[^;]*\bFROM Staging WHERE kind = '{name}';", promotions[1]), name
assert " BLOB" not in sql.upper(), "Byte payloads must not enter the metadata database"
a = "{http://schemas.android.com/apk/res/android}"
manifest = ET.parse(root / "client/androidApp/src/main/AndroidManifest.xml").getroot()
provider = next(p for p in manifest.findall("application/provider") if p.get(a + "name") == "androidx.core.content.FileProvider")
assert provider.get(a + "exported") == "false"
assert provider.get(a + "grantUriPermissions") == "true"
paths = ET.parse(root / "client/androidApp/src/main/res/xml/attachment_paths.xml").getroot()
assert [(p.tag, p.get("path")) for p in paths] == [("cache-path", "attachments/")]
permissions = {p.get(a + "name") for p in manifest.findall("uses-permission")}
assert not permissions.intersection({"android.permission.READ_EXTERNAL_STORAGE", "android.permission.WRITE_EXTERNAL_STORAGE", "android.permission.MANAGE_EXTERNAL_STORAGE"})
print("C5 real collaboration recordings, metadata-only cache and private picker/viewer boundary: OK")
