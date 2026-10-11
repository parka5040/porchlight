#!/usr/bin/env bash
# k6: isolated requested-live-heap measurement; full CI only, normal Cargo test target.
set -euo pipefail
umask 022
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
. scripts/env.sh
mkdir -p var/gates
cd server
# The workspace forbids unsafe. Cap lints at warn ONLY for the isolated probe
# target's reviewed System allocator forwarding, not core or dependencies. Lint
# diagnostics are retained in the JSON log. Ordinary targets remain unsafe-forbid
# and workspace lint policy/manifests are unchanged.
# Cargo chooses rustc, dependencies and artifact; JSON identifies the exact executable.
CARGO_NET_OFFLINE=true cargo rustc -p porchlight-core --test allocation --release --locked \
  --message-format=json -- --cfg porchlight_alloc_probe --check-cfg 'cfg(porchlight_alloc_probe)' \
  --cap-lints=warn > "$root/var/gates/k6-alloc-artifact.jsonl"
python3 - "$root/var/gates/k6-alloc-artifact.jsonl" <<'PY'
import json
import subprocess
import sys
from pathlib import Path
artifacts = [json.loads(line) for line in Path(sys.argv[1]).read_text().splitlines()]
binaries = [a['executable'] for a in artifacts if a.get('reason') == 'compiler-artifact'
            and a['target']['name'] == 'allocation' and a.get('executable')]
if len(binaries) != 1:
    raise SystemExit('expected exactly one Cargo allocation-test executable')
subprocess.run([binaries[0], 'ingest_alloc::measured_peaks_and_camera_admission', '--exact', '--ignored', '--nocapture', '--test-threads=1'], check=True)
PY
