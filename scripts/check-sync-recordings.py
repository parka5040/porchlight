#!/usr/bin/env python3
"""Size/shape guard; live Rust tests independently regenerate and compare handler transcripts."""
import gzip
import json
from pathlib import Path

root = Path(__file__).resolve().parents[1]
folder = root / "client/shared/src/jvmTest/resources/sync-recordings"
files = sorted(folder.glob("*.json.gz"))
assert {p.name for p in files} == {"1575937.json.gz", "1575938.json.gz"}
size = sum(p.stat().st_size for p in files)
assert size <= 5_000_000, f"Recordings exceed 5 MB: {size}"
for path in files:
    recording = json.loads(gzip.decompress(path.read_bytes()))
    assert recording["schema"] == 1
    assert recording["seed"] == int(path.name.split(".")[0])
    steps = recording["steps"]
    assert len(steps) == 46
    assert steps[0]["operation"] == "initial"
    assert steps[-1]["operation"] == "purge"
    for step in steps:
        assert len(step["members"]) == 2
        for member in step["members"]:
            responses = member["responses"]
            assert responses["/v1/snapshot"]["status"] == 200
            assert all(reply["status"] in (200, 403, 404) for reply in responses.values())
            assert "cursor_expired" not in member["sse"], "Expired streams do not exercise targeted sync"
            if step["operation"] == "edit":
                assert "event: item.changed\n" in member["sse"]
                assert "event: caught_up\n" in member["sse"]
            if step["operation"] == "comments_continuation":
                assert any("comments_cursor=" in key for key in responses)
print(f"Server sync recordings: 2 seeds, 46 steps/member, {size} bytes (<= 5000000)")
