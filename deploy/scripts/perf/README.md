# Household performance harness (controller-run)

Python 3 standard library only. Run **only on an explicitly disposable synthetic household**. There is no
cleanup/purge command: keep the private output directory for resume, passwords and audit. No SQL writes, Docker
configuration changes, deployment, signing, or production install/upgrade are performed by these tools.

## Contracts and limitations

- `seed-household.py plan|apply|verify`: default seed 42, fixed anchor `2026-11-13`. Plan prints only counts and a
  plan hash and does not create files. Keep tier/scale/seed/anchor unchanged on resume. Initial: six synthetic
  users, 2,000 items (100 series), 5,000 comments, 50 lists/1,000 entries, 200 small text/PNG attachments. Daily
  series dominate, with weekly/monthly/completion-based series; fixed/floating/all-day anchors in a DST zone
  span spring/autumn transitions. Approximately 20,000 **derived rows**, not an asserted exact occurrence count.
  Mature multiplies items/comments/lists/entries/series by ten, uses 1,000 attachments, and adds one five-year
  daily task and a ten-entry per-occurrence checklist. `--scale` multiplies bulk counts (six users remain fixed).
- All output must be below a real `var/` directory; inside a checkout it must be that checkout's `var/`.
  Symlink output paths are refused. Files containing runtime data are 0600. Passwords are generated locally with
  `secrets`, saved under `--out/passwords/` **before** `porch users add`, and passed only in the CLI child's
  `PORCHLIGHT_PASSWORD` environment. Nothing prints passwords, tokens, keys, enrollment URIs or raw errors.
- Apply takes an output-directory lock and fsyncs intent/done JSONL records. Done steps are skipped. Ambiguous
  creates reconcile by deterministic username/title/list name/entry label/comment body/attachment filename;
  attachments use a private `porch export --member` metadata inventory because there is no attachment-list CLI.
  Do not run independent seeders with the same seed/tier against the same household. Do not lose the passwords
  directory or modify/delete seeded objects while resuming. A changed plan must use a new empty household.
- `verify` reads users/items/lists, paginated comments, list details, and a member export **through porch**; writes
  `verified.json` with actual attachment/occurrence/completion/check counts. Derivation is asynchronous: repeat
  verify after `porch derive status --json` settles. Export may be expensive; never run verification during load.
- **History gap:** `porch occurrences complete`, `reopen`, and `set-status ID skipped` take versions, but no
  completion/skip timestamp. Past item anchors are allowed. Mature really calls complete/check on available past
  occurrences (up to 1,825 distinct completions and 1,825 checks, rotating ten entries); timestamps are **now**,
  not a fabricated five-year activity history. Old rows require explicit `--version` because the CLI's automatic
  version lookup searches only the bounded snapshot; the seeder supplies versions from occurrences list.
  `history-coverage.json` records availability versus requested
  counts. Five-year daily COUNT=1825 is below the 10,000-row per-series derivation cap, but live availability is
  not proven offline. Snapshot keeps a −180/+548-day window; old history still exists in member export. Do not
  describe an old DTSTART or present-day completions as authentic historical completion timestamps.
- `devices.sh` wraps `devices.py`. It round-robins synthetic users and calls `porch dev-init` with their saved
  passwords. Despite its name, `server/cli/src/dev_init.rs` has **no development-install gate**: it mints normal
  enrollment URIs over the agent socket and completes the production pinned-CA enrollment protocol. Therefore it
  works on a non-dev install if the advertised enrollment listener is reachable from an allowed LAN address.
  It saves `ca.crt`, `client.crt`, `client.key`, `session.token`, and `enrollment.json`, matching the M5/M8 device
  format. Completed directories are retained; pending attempts resume with the original password/identity.
  If credentials were issued but files are incomplete, inspect/revoke that device rather than deleting keys.
- Each device also gets a 0600 `target.json` with **both label and exact HTTPS base URL**. Load refuses every
  mismatch before opening any network connection. Provisioning a different target requires new device dirs;
  do not edit target files to bypass the guard. Certificates verify the household CA and hostname; no insecure
  SSL switch, redirects, automatic external URL fetching, or URL credentials are supported.
- Hard load ceilings: 50 actual HTTP requests/s, 256 concurrent connections, 3,600 seconds. Options can only
  lower these limits. All setup, auxiliary version reads, and stream reconnects share the rate limiter. Ladder
  steps are paced **logical operation** rates; auxiliary reads count toward the HTTP ceiling, so achieved work
  can be lower. At most one operation/device is outstanding; saturated ticks are dropped, never queued for a
  burst. Per-step histograms and dropped-operation counts are in JSON. Defaults are intentionally modest.
- Ladder mix: 30% board/agenda, 20% item/occurrences/comments, 15% lists, 10% notifications, 10% writes
  (complete/reopen, comment, list check/uncheck), 5% search, 5% snapshot, 5% attachment. Reads include full bodies;
  auxiliary version reads are reported separately. There are no silent HTTP retries. 409/429 count as errors;
  the report rejects 4xx/transport failures as invalid workload samples, even when the 5xx budget itself passes.
  Writes use fresh UUID Idempotency-Key and observed If-Match versions. Stop on consecutive 5xx/transport errors
  (default five). Foreground mirrors snapshot → all board pages → all agenda pages, −180/+548 days.
- SSE framing handles byte boundaries, CRLF, multiline data, IDs, comments/heartbeats and reconnect cursors.
  Edit-fanout waits for all streams' ready frames before editing; `--heavy-recurring-edit` changes RRULE so it
  measures a recurring edit rather than just a title patch. Missing deliveries are failures, not omitted samples.
  There is no server commit timestamp: ack→event is a lower bound (clamped at zero), request-start→event is a
  conservative upper bound used for the budget. Both are saved, as are per-stream samples. Tickle payloads carry
  entity IDs rather than edit versions; run against a quiescent seeded household, not concurrent external edits.
- Slow readers deliberately consume headers but no body. They report header status, held duration and whether
  failure preceded headers. TLS/kernel buffering prevents proving a peer close without consuming body; the
  tool explicitly marks this **not observable**, rather than falsely claiming a server eviction. Small files may
  fit buffers completely: use mature snapshots and observer resource curves to evaluate actual backpressure.
- Observer runs **on the Docker host**, every five seconds when collection fits in that interval. Docker stats
  are filtered by Compose project; inspect requests only `.State.Pid`. Cgroup v2 OOM counters are read if permitted.
  Postgres uses fixed `docker exec … psql -U postgres -d porchlight -XAt` queries, never query text/parameters.
  `pg_stat_statements` is optional; missing extension/permissions are recorded as unavailable, not zero. Do not
  enable extensions or restart Postgres just for this tool without a separate controller decision.
- `report.py` accepts repeated `--load` to cover all scenarios. Each endpoint/ladder phase is checked separately;
  an overall average cannot hide a slow 50-rps phase. It includes only observer samples in the run time window
  (combined runs use their enclosing time window). Absolute OOM counters conservatively include pre-run OOMs.
  Missing budgets/required OOM coverage produce **INCOMPLETE**, exit 2; budget failure exits 1, complete pass 0.
  Keep individual reports too when comparing tiers. Histograms use upper-rounded 1% logarithmic ms buckets.

## Routine edit fanout comparison

`fanout-compare` defaults to **12 distinct enrolled devices**, **20 edits per protocol**, and a 600-second
whole-run wall-clock cap (setup and both protocols included). `--streams M --edits K` lowers/changes the sample;
provision at least M devices first. `--max-concurrency` must be at least **2 × M**, because every device holds
one stream plus one sequential read connection. The same shared HTTP rate limiter includes every full-refresh
page, continuation, writer/version read and stream connection. Rate-cap wait is included in measured latency;
do not interpret a rate-limited run as the server's unconstrained capacity.

The same devices and member-specific −180/+548-day windows are reused, legacy first, targeted second. The edit
mix rotates title PATCH, occurrence complete, RRULE PATCH and comment POST (five each at defaults). Only seeded
items readable by all devices and writable by the first are chosen; fail rather than omit unauthorized/missing
work. This **modifies the disposable seed**: it does not revert writes or pretend both protocols began at an
identical database snapshot. Stop competing clients, seeders and edits; wait for seed derivation before running.

- Legacy streams **omit** `Porchlight-Capabilities`. Each delivered content frame triggers snapshot → every
  household-board page → every own-member agenda page, matching `foreground`; frames are not coalesced.
- Targeted streams advertise `Porchlight-Capabilities: item-sync-v1` on every connect/reconnect. Item, occurrence,
  derived and item-invalidated frames fetch `/v1/items/{id}/sync` with the device's own member/window and
  `include=collaboration`; every agenda/comment continuation is followed with continuation-only parameters.
  Comments and inbox tickles refetch exact `ids`; attached lists/attachments fetch metadata via their item bundle,
  standalone lists fetch their detail, and settings/member/feed/suggestion frames use their scoped reads. No
  binary attachment download is performed. Evictions need no content reads. A terminal resnapshot performs a
  full foreground refresh and reconnects from that snapshot's cursor.
- `/v1/me` is checked on every device. Missing `item-sync-v1` (including a 0.1.7 server) records targeted
  **NOT AVAILABLE**, preserves the legacy measurements and never probes `/sync`. The report is INCOMPLETE, not
  a claimed targeted budget pass. HTTP/decoding/continuation errors and missing deliveries fail the run.
- Every request advertises `Accept-Encoding: gzip`; stdlib zlib incrementally decodes compressed bodies with
  size/EOF checks. **Wire bytes** are HTTP response entity bytes (compressed when the server selects gzip),
  excluding headers, chunk framing, TLS, request bodies and TCP overhead. Decoded bytes are counted separately;
  delivered SSE body chunks are included. This is not packet capture or total network interface traffic.
- Raw JSON records each edit/device's request count, wire/decoded bytes, every delivered frame's timing bounds,
  write acknowledgement, per-device completion and all-devices completion, plus edit totals and separate stream
  setup/whole-protocol totals. Writer/version reads count against the writer's edit totals. Protocol/edit wall
  timestamps allow matching the host observer samples. Report means use complete edits only; unfinished edits
  remain explicit failures, never silent omissions.
- There is no server commit timestamp. Commit→event is reported as ack→event lower and request-start→event
  upper bounds, not a fabricated exact clock. Completion uses the conservative request-start bound. All devices
  must receive the edit's native frame (or a superseding terminal reset), consume every subsequent frame, and have
  no observed pending derivation. Reset samples record `delivery_kind=resnapshot` and await a fresh stream ready.
  A **global** quiet period (`--settle-seconds 1`, configurable 0.1–30) follows all reads before proceeding; that
  wait is excluded from the last-read completion latency. Quiescence is **not a server completion fence**:
  delayed notifier/load-worker events beyond the quiet interval can belong to later samples. Increase the quiet
  interval and inspect observer backlog/derivation curves when investigating delayed work.

Targeted budgets: nearest-rank p95 **≤ 100,000 wire bytes/device/edit** and **≤ 2,000 ms until all devices'
follow-up reads complete** on LAN. The table compares requests/edit and decimal wire/decoded MB/edit summed
across all devices, and all-device completion p50/p95. A full budget report still needs the other scenarios and
host observer coverage. To report only this comparison (other gates remain explicitly INCOMPLETE):

```bash
python3 -I "$tools/report.py" --load "$out/results/compare.json" \
  --observe "$out/results/observe.jsonl" --out "$out/results/compare.md"
```

Both exact recipes below provision 12 devices, allow 32 concurrent connections, and include this comparison.
All existing synthetic-only, target-binding, private-output and controller/owner production safety rails apply.

## Laptop dev stack

Run in bash from the **main checkout**, not a `var/wt` copy (the dev-stack helper deliberately refuses copies).
These commands do not reset an existing stack. Stop first if it contains anything you cannot discard.

```bash
umask 022
. scripts/env.sh
scripts/dev-stack.sh up
(cd server && CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true cargo build --locked -p porchlight-cli)
porch="${CARGO_TARGET_DIR:-$PWD/server/target}/debug/porch"
tools="$PWD/deploy/scripts/perf"
out="$PWD/var/perf/laptop"
umask 077
mkdir -p "$out"
seed=(python3 -I "$tools/seed-household.py" --porch "$porch" \
  --socket "$PWD/var/run/agent.sock" --token-file "$PWD/var/bootstrap.pat" \
  --out "$out/seed" --tier initial --seed 42 --anchor 2026-11-13)
"${seed[@]}" plan
"${seed[@]}" apply
"${seed[@]}" verify
base="https://localhost:${LAN_PORT:-8443}"
label=laptop-dev
version=$(<VERSION)
bash "$tools/devices.sh" --porch "$porch" --socket "$PWD/var/run/agent.sock" \
  --token-file "$PWD/var/bootstrap.pat" --seed-dir "$out/seed" --out "$out/devices" \
  --count 12 --target-label "$label" --base-url "$base" --enroll-addr "localhost:${ENROLL_PORT:-8444}"
project=porchlight-dev
pg=$(docker ps --filter "label=com.docker.compose.project=$project" \
  --filter label=com.docker.compose.service=postgres --format '{{.ID}}')
```

`scripts/dev-stack.sh up` initializes PKI/household only if absent, captures init output privately, extracts the
bootstrap PAT to **var/bootstrap.pat (0600)**, and removes var/init.json. The published agent socket is
**var/run/agent.sock**. The bootstrap PAT has admin scope. Do not print either init JSON or the PAT. Custom
published ports must match the running dev stack; the hostname must be in its certificate SANs.

With the variables above, run the following block on the laptop (also used on the production host below):

```bash
mkdir -p "$out/results"
bash "$tools/observe.sh" --project "$project" --pg-container "$pg" --database porchlight \
  --out "$out/results/observe.jsonl" --duration 1200 >"$out/results/observer.log" 2>&1 &
observer=$!
trap 'kill "$observer" 2>/dev/null || true' EXIT
load=(python3 -I "$tools/load-household.py" --devices "$out/devices/devices.json" \
  --base-url "$base" --target-label "$label" --client-version "$version" \
  --max-rps 50 --max-concurrency 32)
"${load[@]}" ladder --steps 1,10,25,50 --step-duration 15 --duration 120 --out "$out/results/ladder.json"
"${load[@]}" foreground --duration 120 --out "$out/results/foreground.json"
"${load[@]}" edit-fanout --streams 6 --duration 30 --out "$out/results/fanout.json"
"${load[@]}" edit-fanout --streams 6 --heavy-recurring-edit --duration 30 --out "$out/results/heavy.json"
"${load[@]}" slow-readers --readers 4 --duration 30 --out "$out/results/slow.json"
"${load[@]}" sse-hold --streams 6 --duration 150 --out "$out/results/hold.json"
"${load[@]}" fanout-compare --streams 12 --edits 20 --duration 600 --out "$out/results/compare.json"
kill "$observer" 2>/dev/null || true
wait "$observer" 2>/dev/null || true
trap - EXIT
python3 -I "$tools/report.py" --load "$out/results/ladder.json" --load "$out/results/foreground.json" \
  --load "$out/results/fanout.json" --load "$out/results/heavy.json" --load "$out/results/slow.json" \
  --load "$out/results/hold.json" --load "$out/results/compare.json" --observe "$out/results/observe.jsonl" --out "$out/results/summary.md"
```

Use `set -e` in a controller shell if a failed scenario should stop subsequent scenarios; raw failed-run JSON is
still written. For larger ladders enroll more synthetic devices first (for example `--count 24`) and raise only
`--max-concurrency` to match; never raise the hard caps. For mature use a **new empty household** and a new output
root with `--tier mature`. `--scale 0.01` is a useful smoke run before full tiers. Do not run competing seed tiers
or measurement passes concurrently. A 150-second hold is long enough to see the production 120-second heartbeat.

## Production host via SSH (controller/owner only)

No install/upgrade command is needed. Copy only public harness code. Machine values come from the owner's
already-provisioned gitignored `.env`; the API URL is supplied at runtime and must match an advertised LAN
endpoint/SAN, not Funnel. The PAT remains on the server throughout; do not use `enroll-uri` here because its
purpose is to return an enrollment URI to the laptop. Instead reuse its active-release/socket/token lookup.

From the laptop checkout in bash:

```bash
umask 077
set -a
. ./.env
set +a
: "${PORCHLIGHT_DEPLOY_HOST:?}" "${PORCHLIGHT_DEPLOY_DATA_MOUNT:?}"
read -r -p 'Production LAN HTTPS base URL: ' PERF_PROD_URL
remote_root="$PORCHLIGHT_DEPLOY_DATA_MOUNT/porchlight"
printf -v qroot '%q' "$remote_root"
printf -v qbase '%q' "$PERF_PROD_URL"
ssh -o BatchMode=yes "$PORCHLIGHT_DEPLOY_HOST" \
  "umask 077; mkdir -p $qroot/var/perf/tools"
tar -C deploy/scripts/perf -cf - . | ssh -o BatchMode=yes "$PORCHLIGHT_DEPLOY_HOST" \
  "umask 077; tar -xf - -C $qroot/var/perf/tools"
ssh -o BatchMode=yes "$PORCHLIGHT_DEPLOY_HOST" "bash -s -- $qroot $qbase" <<'REMOTE'
set -euo pipefail
umask 077
root=$1
base=$2
tools="$root/var/perf/tools"
out="$root/var/perf/initial"
mkdir -p "$out"
tmp=$(mktemp -d "$out/pat.XXXXXX")
trap 'rm -rf -- "$tmp"' EXIT
# If init.json was retired, pre-provision an admin PAT file on the server and replace
# the first argument below with that path, using its raw 0600 contents instead of JSON.
python3 -I - "$root/init.json" "$tmp/token" <<'PY'
import json, os, sys
fd = os.open(sys.argv[2], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, 'w') as f:
    f.write(json.load(open(sys.argv[1]))['bootstrap_token'])
PY
active=$(readlink -- "$root/release-current")
version=${active##*/}
release=$(readlink -f -- "$root/release-current")
porch="$release/porch-$version-x86_64-linux-musl"
[[ -x $porch && -S /run/porchlight/agent.sock ]]
seed=(python3 -I "$tools/seed-household.py" --porch "$porch" --socket /run/porchlight/agent.sock \
  --token-file "$tmp/token" --out "$out/seed" --tier initial --seed 42 --anchor 2026-11-13)
"${seed[@]}" plan
"${seed[@]}" apply
"${seed[@]}" verify
label=production-lan
bash "$tools/devices.sh" --porch "$porch" --socket /run/porchlight/agent.sock \
  --token-file "$tmp/token" --seed-dir "$out/seed" --out "$out/devices" \
  --count 12 --target-label "$label" --base-url "$base"
# Token can be discarded now: measurement uses only synthetic members' mTLS devices.
rm -rf -- "$tmp"
trap - EXIT
project=porchlight
pg=$(docker ps --filter "label=com.docker.compose.project=$project" \
  --filter label=com.docker.compose.service=postgres --format '{{.ID}}')
mkdir -p "$out/results"
bash "$tools/observe.sh" --project "$project" --pg-container "$pg" --database porchlight \
  --out "$out/results/observe.jsonl" --duration 1200 >"$out/results/observer.log" 2>&1 &
observer=$!
trap 'kill "$observer" 2>/dev/null || true' EXIT
load=(python3 -I "$tools/load-household.py" --devices "$out/devices/devices.json" --base-url "$base" \
  --target-label "$label" --client-version "$version" --max-rps 50 --max-concurrency 32)
"${load[@]}" ladder --steps 1,10,25,50 --step-duration 15 --duration 120 --out "$out/results/ladder.json"
"${load[@]}" foreground --duration 120 --out "$out/results/foreground.json"
"${load[@]}" edit-fanout --streams 6 --duration 30 --out "$out/results/fanout.json"
"${load[@]}" edit-fanout --streams 6 --heavy-recurring-edit --duration 30 --out "$out/results/heavy.json"
"${load[@]}" slow-readers --readers 4 --duration 30 --out "$out/results/slow.json"
"${load[@]}" sse-hold --streams 6 --duration 150 --out "$out/results/hold.json"
"${load[@]}" fanout-compare --streams 12 --edits 20 --duration 600 --out "$out/results/compare.json"
kill "$observer" 2>/dev/null || true
wait "$observer" 2>/dev/null || true
trap - EXIT
python3 -I "$tools/report.py" --load "$out/results/ladder.json" --load "$out/results/foreground.json" \
  --load "$out/results/fanout.json" --load "$out/results/heavy.json" --load "$out/results/slow.json" \
  --load "$out/results/hold.json" --load "$out/results/compare.json" --observe "$out/results/observe.jsonl" --out "$out/results/summary.md"
REMOTE
```

If bootstrap init.json was removed, use an existing **server-side** regular 0600 admin PAT file: replace the
Python extraction block with `cp -- "$server_pat_file" "$tmp/token"; chmod 600 "$tmp/token"`, after validating
`[[ -f $server_pat_file && ! -L $server_pat_file && $(stat -c %a "$server_pat_file") == 600 ]]` on the server.
Never scp a PAT, synthetic passwords, device keys or session tokens back to the laptop. Results JSON/JSONL and
summary markdown contain only metrics, labels, numeric query IDs and counters and may be retrieved privately
under laptop `var/perf/production/`. No real-data operation is authorized by this recipe.

## Offline verification

```bash
python3 -I deploy/tests/perf-harness.py
bash -n deploy/scripts/perf/{devices,observe}.sh
shellcheck deploy/scripts/perf/{devices,observe}.sh  # if installed
bash scripts/ci.sh --sandbox > var/perf-harness-ci.log 2>&1; printf '%s\n' "$?"
```

The fake-porch test exercises apply/verify, done-step resume, and recovery after an attachment committed without a
journal done line. Other tests cover deterministic hashes, refusal guards, histogram ranks, recorded SSE bytes,
HTTP keep-alive/required headers, consecutive-error abort, fixed observer queries, and report thresholds/missing
coverage. `perf_harness2.py` is included by that same test entry point: recorded gzip bytes/decoded accounting,
truncated/oversized bodies, byte-recorded continuation pages, exact-ID routing, missing capability, both protocol
edit mixes, every-frame reads, and targeted budget thresholds. Offline tests do **not** prove a real enrollment,
real history generation, resource budgets or throughput.
