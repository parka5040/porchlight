#!/usr/bin/env bash
# Isolate requested-live-heap accounting from other lib tests and server activity.
set -euo pipefail
umask 022
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
. scripts/env.sh
mkdir -p var/gates
cd server
# As with check-ingest-alloc.sh, cap lints only on the test executable for
# reviewed System allocator forwarding; ordinary workspace code forbids unsafe.
CARGO_NET_OFFLINE=true cargo rustc -p porchlight-api --lib --profile test --locked \
  --message-format=json -- --cfg porchlight_snapshot_probe \
  --check-cfg 'cfg(porchlight_snapshot_probe)' --cap-lints=warn \
  > "$root/var/gates/snapshot-alloc-artifact.jsonl"
python3 -I - "$root/var/gates/snapshot-alloc-artifact.jsonl" <<'PY'
import json
import subprocess
import sys
from pathlib import Path
artifacts = [json.loads(line) for line in Path(sys.argv[1]).read_text().splitlines()]
binaries = [a['executable'] for a in artifacts if a.get('reason') == 'compiler-artifact'
            and a['target']['name'] == 'porchlight_api' and a.get('executable')]
if len(binaries) != 1:
    raise SystemExit('expected one Cargo snapshot allocation-test executable')
for test in ['snapshot_cost', 'router_probe::shipping_gzip_cost',
             'item_sync_probe::bounded_bundle_cost_and_continuation_equivalence']:
    subprocess.run([binaries[0], 'routes::reads::prerollout_srv_snapshot::fixture::' + test,
                    '--exact', '--nocapture', '--test-threads=1'], check=True)
PY
