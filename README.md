# Porchlight

A self-hosted task, chore, list and calendar system for one household, running on a home server and reachable
only by enrolled devices over mTLS.

| Path | Contents |
| --- | --- |
| `docs/specs/` | Design spec and architecture diagram |
| `docs/plans/` | Master plan and milestone plans: the owner's working history, kept local (gitignored) |
| `docs/operations/` | Runbooks (milestone gate logs stay local) |
| `design/` | Screen mockups and app icon |
| `server/` | Rust workspace: `core`, `db`, `api` (`porchlight` binary), `cli` (`porch` binary) |
| `client/` | Kotlin Multiplatform Android + Linux desktop apps |
| `deploy/` | Compose files, Dockerfiles, install script |
| `scripts/` | `ci.sh` (local CI), `snapshot.sh` (source snapshots) |
| `var/` | Local runtime state and secrets (never snapshotted) |
| `.env.example` | Machine profile: copy to `.env` (gitignored) for toolchain paths and the server's host, data mount and LAN address |

Copy `.env.example` to `.env` and fill it in: nothing specific to a machine lives in the repo, and `ci.sh` fails if a
value from `.env` appears in a committed file. Check everything: `scripts/ci.sh` (add `--full` for slow checks).

## Calendar import and read-only feeds (S8a / M7 server)

```bash
. scripts/env.sh
# MEMBER_ID accepts a username or UUID; SOURCE_ID is a UUID. Global --socket/--token-file select the agent connection.
porch --json import ics calendar.ics --member "$MEMBER_ID" --name school --scope school-file --preview
porch --json import ics calendar.ics --member "$MEMBER_ID" --name school --scope school-file
porch --json feed add --name school --url "$FEED_URL" --member "$MEMBER_ID"
porch --json feed list --limit 50
porch --json feed refresh "$SOURCE_ID" --if-match "$VERSION"
porch --json feed rm "$SOURCE_ID" --if-match "$VERSION"
# Controller: isolated Compose project; no shared dev DB changes, diagnostics stay private.
bash deploy/scripts/m7-demo-ics.sh
```

`--scope` is a stable import identity namespace (CLI and HTTP default to `import`); identical UIDs in another scope
or subscription cannot collide. Preview makes no calendar/source/history changes; commit repeats normal caller ACL
and validation checks. One-off imports remain editable. App-capable member targets own their items (default private).
Dependent targets instead have the importing caller as owner, the dependent as subject, and default household visibility
(R49a); feeds use the creating super admin as owner. `--member` accepts username, display name or UUID; ambiguous
names fail with candidate IDs; an exact username wins over display-name matches. Source visibility values are exactly `private | household`; `selected`/other values
return a 422 visibility field error. One-off `--visibility` controls new items; existing editable items keep their
visibility, and changing only this request setting does not rewrite them.
The API accepts a JSON envelope or UTF-8 `text/calendar`/`text/plain` body (target/name/scope/preview in query parameters).
Maximum ICS size is 5 MiB; source names allow 1–196 characters (the `ics:` badge must fit); import scopes independently allow 1–200 characters without controls. Invalid events report `InvalidEvent` per UID and are skipped in feeds and one-off imports while good events apply; an invalid UID's old items remain untouched. Unchanged normalized events cause no item/occurrence writes; summaries separate created/updated/unchanged/removed (deleted remains a compatibility alias). Reimports update existing identities, retained EXDATE/cancellations skip occurrences and
RECURRENCE-ID moves override the original occurrence. Unsupported rules import discrete expanded instances.

Feed administration requires app super admin or agent admin-PAT; CLI imports require a write/admin PAT. Feed items
are read-only in the app, count as busy appointments, and remain agent-editable. `refresh` returns 202 and queues a
fetch; each enabled source is polled when due (household-local 02:30 nightly by default; fetch/parse failures back off 5, 10, 20… minutes, capped at the refresh interval; success resets the backoff). App-capable member feeds default private, dependent/household feeds default household; source visibility changes propagate to all their items. Removing a subscription soft-deletes all its items and retires its UID mappings. Re-adding creates fresh items, not duplicate live events. A failed/malformed fetch never deletes events; incomplete responses suppress removal for affected UIDs (globally only when identity/expansion is lost) and report `partial` with bounded/sanitized warnings, keep validators and use the normal schedule. `ExpansionLimited` clears validators to retry expansion later; `DurationNormalized` is informational, not partial. Feed PUT resets backoff/warnings and queues an immediate fetch. Successful removal soft-deletes events; expansion-window
absence never deletes historical instances outside that window. HTTPS is required unless the source explicitly enables
HTTP; all resolved addresses/redirect hops are vetted against nonpublic ranges and connections are DNS-pinned.
Agents may add assignees: rename/visibility/URL/interval changes preserve them, but retargeting clears them, as does applying a changed remote event; unchanged refreshes preserve agent corrections. These rules also apply to raw source configuration SQL.
Household-target feeds create items with **no assignees**: household visibility permits calendar reads, while R2 agenda/busy
membership follows the owner, not every household member. New members can immediately see them without synchronization;
use a member-target feed to make a calendar that member's busy time. Removing the creating admin disables their feed
with `member_removed`; recreate it under an active admin. The gate owns `porchlight-m7icsdemo`, its private env/PAT and
separate ports/state; normal exit tears down its DB volume, `--keep` retains only that isolated project for inspection.

## Availability and planning (S8b)

`porch schedule suggest [--member <dependent-uuid>] --json` proposes your agenda work (or a dependent's as super admin), without moving work. Empty availability disables load tracking and EDF proposals.
`porch schedule list [--member <uuid>] --json` returns proposals/history plus occurrence versions.
Accept with `porch schedule accept <suggestion-id> --version <occurrence-version> --json`;
dismiss with `porch schedule dismiss <suggestion-id> --version <suggestion-version> --json`.
`porch schedule availability <member> --json` and `set-availability <member> --file <windows.json> --version <collection-version> --json`
read/replace local weekday windows (new empty collection version is 0). `porch schedule candidates <item> --json`
ranks unassigned dependent appointments. All commands use the normal private PAT file/agent socket configuration.

Controller Docker/HTTP/CLI gate (future-dated fixtures): `deploy/scripts/m7-demo-planning.sh [--keep]`. See
[planning contract](docs/specs/planning.md); notifications/snapshot/shared read integration awaits S4b, clients C6.

## Client shell (C1a)

```bash
umask 022
. scripts/env.sh
cd client
./gradlew --offline --console=plain :shared:jvmTest :androidApp:assembleDebug :desktopApp:jar :desktopApp:createDistributable
```

`design/tokens.json` and `VERSION` generate Kotlin tokens/BuildInfo and the Android icon background before
compilation; generated files live under `client/build/generated/` and are never edited. The tests check token
contrast for real drawing roles (brand fills are separate from accent text), the single member-fill list,
bounded tab stacks/back/resume, unique-sender portal wiring, rendered theme/card colours, inset padding,
settings label-in-name and checked selection, bundled fonts, 48dp controls and 200% navigation labels.
`ci.sh --sandbox` includes these headless JVM tests offline alongside server checks, but not client packaging.
Normal host CI may resolve dependencies online. The desktop icon resource is generated from the original
`design/icon/porchlight-icon-512.png`; there is no checked-in resource copy. Android uses edge-to-edge, safe
drawing insets, a NoActionBar theme, and a separate transparent monochrome launcher layer.
Settings/resume use per-device DataStore (Android app files; desktop `$XDG_DATA_HOME/porchlight`, otherwise
`~/.local/share/porchlight`). Theme follows Android or the desktop portal, with a dark fallback on portal absence.
See `docs/design-pending.md` for intentionally unfinished content. Both entry points now wire the trust gate,
Settings → Account, and persistent services; Android checks device credentials before enrollment/login and relocks
on backgrounding. Trust/onboarding recovery and limited-reminder persistence are documented in
[`client-trust.md`](docs/operations/client-trust.md), including R24 Secret Service activation on niri (ksecretd).

## Lists (S7a / M6 server)

`porch --socket var/run/agent.sock --token-file var/bootstrap.pat --json lists` supports
`list`, `show`, `add`, `add-entry`, `check`, `uncheck`, `clear`, `rm`, `edit`, `edit-entry`, `rm-entry`, and `viewers`.
Use `porch lists <command> --help` for flags. Writes fetch current versions unless `--version` is supplied;
stale writes return the authorized current resource. For example, `porch lists add Groceries`, then
`porch lists add-entry groceries "Oat milk" --qty 2`, `porch lists check <entry-id> --list-id <list-id>`, and
`porch lists clear groceries`. List selectors accept UUID or an unambiguous case-insensitive name; duplicate names
require UUIDs. The global socket/token-file flags apply to every command.

Persistent lists separate checked entries into `got_it`; clear archives them without deleting history. Clear's
If-Match covers list metadata/ACL, **not the checked set**: it clears every checked entry at execution time, including
checks since the client's last fetch. Checks bump entry versions only. Empty PATCH and clear of an empty Got it
write no versions, revisions or tickles.
Attached `--mode per-occurrence --item-id <item-id>` lists require a recurring item and `--occurrence-id` for
check/uncheck. Each occurrence starts fresh; templates and existing checks follow split descendants ("this and
following") using S3's `split_from_item_id` lineage. Attached permissions inherit the original parent item.
Standalone readers may add/edit/reorder/archive entries, check/uncheck and clear; only owner/super admins may
rename, change visibility/viewers/owner or delete. App super admins follow visibility, agents see all.
Deleted parents exclude attached lists from default collection/snapshot/search. Owners/super admins can discover
cleanup candidates with `GET /v1/lists?include_orphaned=true` or `porch lists list --include-orphaned`, then delete
by UUID using the page's explicit `--version` (204). Others never see orphans. Lists do not cascade-delete.
Product links use HTTP(S); moves use `--after-id` (predecessor) / `--before-id` (successor).
`check` / `uncheck` accept optional `--list-id` to avoid collection scans; `edit-entry` / `rm-entry` require it.
`edit-entry` supports qty/unit/url/notes (`--clear-qty`, `--clear-url`); `edit --owner-id` transfers ownership.
Every command accepts global `--json`; human output is escaped pretty JSON, with a golden fixture.

Controller-only live gate: `deploy/scripts/m6-demo-lists.sh var/clients/desktop var/bootstrap.pat` against the
running dev stack and a built `porch` on PATH (or set `PORCH_BIN`). Sandbox CI runs the SQL, HTTP, permission-matrix
and CLI tests and gate syntax checks. `/v1/snapshot` includes lists in its single repeatable-read view/cursor;
`/v1/search` searches live list names/entry labels/notes with authoritative ACLs and signed `(source,id)` pages.
List checks emit outbox tickles only: the closed notification kind contract has no list-check event.
No client screens are part of this workstream.

## Generated client contract (C2a)

```bash
umask 022
. scripts/env.sh
(cd client && ./gradlew --offline --console=plain :contract-check:jvmTest)
```

The `contract-check` KMP/JVM module generates the M2 Kotlin client from `docs/openapi.json` into its build directory,
compiles it, and decodes sanitized me/devices/problem fixtures. A server-side equality test records/checks these
against the real M2 handlers and problem serializer (normalizing only volatile IDs/timestamps). Output cleanup
removes stale models, a schema allowlist guards free-form mappings, and the module has a dependency lock.
Sandbox CI runs these checks and shared Android production-source lint. Handwritten production
trust transport remains the only HTTP stack; C2b's generated shared operations delegate through its household-CA-pinned
mTLS coordinator. Shared generation reads the authoritative document directly with C2a's conventions; presence-aware
PATCH, empty-body and search-oneOf mappings remain intentional. See the HTTP contract section below for schema regeneration.

## Client household data (C2b)

```bash
umask 022
. scripts/env.sh
(cd client && ./gradlew --offline --console=plain :shared:jvmTest :shared:testAndroidHostTest :contract-check:jvmTest \
  :androidApp:assembleDebug :desktopApp:jar :androidApp:lintDebug :shared:lintAndroidMain)
CARGO_NET_OFFLINE=true scripts/ci.sh --sandbox
```

`AppServices.data` provides generated-model repository flows and online-only writes, an atomic SQLDelight snapshot
cache, and sync/refetch entry points for C3/C4. Desktop cache lives beside device preferences in the XDG data directory;
Android uses an application SQLite database. Ready/unlock syncs, staging board/agenda with the snapshot; the persisted
cache survives cold starts, PIN relock and session expiry; logout/re-enroll/another identity fence old responses and
clear it. Server recordings are equality-checked in Rust and decoded by the JVM fixture consumer in CI.
See [`client-data.md`](docs/operations/client-data.md) for interfaces, cache semantics and controller smoke checks.

## Calendar, household settings and M4 client gate (C3b)

Calendar uses server-resolved member agendas: desktop Day/Week/Month, Android Day/Week/Agenda; mobile Month is deferred.
Today section order/filter/range and your member color save online with version checks. Theme/start screen stay local;
resume checks cached visibility without networking before the trust/PIN gate. See [client screens](docs/operations/client-screens.md).

```bash
umask 022
. scripts/env.sh
(cd client && ./gradlew --offline --console=plain --max-workers=4 :shared:jvmTest :shared:testAndroidHostTest \
  :androidApp:assembleDebug :androidApp:assembleDebugAndroidTest :desktopApp:jar :androidApp:lintDebug :shared:lintAndroidMain)
CARGO_NET_OFFLINE=true scripts/ci.sh --sandbox
# Controller only, KVM + graphical desktop, API35/API36 cached images:
deploy/scripts/m4-gate.sh   # evidence: var/gates/m4-ci.log
# Full CI includes these same managed-device and packaged launch checks:
scripts/ci.sh --full
```

## Signed releases and recovery (O1 / M8 laptop)

```bash
. scripts/env.sh
scripts/dev-backup-role.sh     # once for existing dev clusters, before migration 0050
scripts/release-keys.sh        # operator only, once; never overwrites var/release
scripts/release.sh             # signed dist/<VERSION>, Docker + offline Gradle
# Desktop from the trusted distribution; server scratch install settings are in the runbook.
bash dist/0.1.0/install.sh desktop --source dist/0.1.0
porchlight-update /local/path/to/new-release
porchlight-update --rollback
# Restore newest dev bundle to isolated scratch stack (requires matching dist + laptop identity).
deploy/scripts/restore-drill.sh --source dist/0.1.0
```

Signed builds include `release.json` and enforce committed minisign/APK signer pins. The owner publishes verified,
clean, pushed builds with `scripts/publish-release.sh <V>` (dry run first); see [Releases](../../releases) and the
[publish runbook](docs/operations/release.md#publish). Signing keys remain on the laptop.

Production/laptop-release `deploy/compose.yml` is separate from the dev stack; no Funnel is enabled on the laptop.
Runbooks: [release](docs/operations/release.md), [install](docs/operations/install.md),
[upgrade/rollback](docs/operations/upgrade.md), [backup/restore](docs/operations/backup-restore.md),
[key custody](docs/operations/keys.md), [CA rollover](docs/operations/ca-rollover.md).
Sandbox CI checks shell/Python syntax and deterministic operations fixtures; real signing/image/restore checks are
operator gates. Recovery/signing keys, stack .env, PKI, and plaintext diagnostics stay under private `var/` only.

## Laptop dev stack (M2 Trust)

```bash
. scripts/env.sh
scripts/dev-stack.sh reset    # DEV TEST DATA ONLY: needed once after an M1 demo populated the DB
scripts/dev-stack.sh up       # postgres -> one-shot migrate -> init if needed -> api + sidecar
scripts/dev-stack.sh dev-init # enroll emulator + desktop; refuses to overwrite existing clients
```

`reset` drops/re-bootstraps/migrates the dev database and removes dev PKI, socket, bootstrap PAT and clients.
It keeps `var/dev.env` and the backup recovery key. Do not use it on data you want to preserve. `init` explicitly
initializes an empty migrated database, refusing an existing `var/server/pki`; `down` stops the dev stack.
The older `scripts/dev-db.sh up|stack|down|psql|reset` commands remain available (`stack` now uses the Trust
sequencing). Its `reset` resets only the database; prefer `dev-stack.sh reset` for M2 so PKI and DB remain paired.

Compose project `porchlight-dev` runs Postgres 18.6, the non-root api, one-shot migrations, and a pinned HAProxy
stand-in for the later Tailscale sidecar. LAN mTLS is on `:8443`, server-auth enrollment on `:8444` (both LAN
reachable), and loopback `127.0.0.1:9445` forwards PROXY v2 to the **unpublished** api Funnel listener `:9443`.
The PAT-authenticated agent socket is `var/run/agent.sock`; household PKI and secrets are in `var/server`.
Database/admin passwords are generated in `var/dev.env`, and init saves its bootstrap PAT to
`var/bootstrap.pat`, all private (0600). The age identity is `var/backup-recovery.key`: protect/back it up.
Nothing secret is printed by these helpers. Do not enable shell tracing or print Compose's expanded config.
The dev helpers require bash; they refuse zsh and interactive sourcing before any side effects.
For administration, prefer `porch --socket var/run/agent.sock --token-file var/bootstrap.pat whoami`:
the file must have no group/other permissions (0600), and a trailing newline is trimmed.
`PORCHLIGHT_TOKEN` remains supported; argv token values are not accepted. Prefer a private `--token-file`.

The Kotlin trust adapters and their integration contract are documented in
[`docs/operations/client-trust.md`](docs/operations/client-trust.md). Controller-only real-client checks are
`scripts/c1b-desktop-e2e.sh` (unlocked Secret Service wallet) and
`scripts/c1b-emulator-e2e.sh <adb-serial>` (dev enrollment listener advertises `ENROLL_HOST=10.0.2.2`). They keep
URI/password inputs private under `var/` and never put them in adb arguments. C2a's app/navigation wiring is active;
the instrumented enrollment probe still tests transport separately. Managed-device suites exclude `@ControllerOnly`;
the live emulator helper substitutes `notAnnotation=org.junit.Ignore` for its explicitly selected enrollment class. The live desktop probe asserts fresh-certificate renewal returns 409 `renewal-too-early` over real mTLS;
local TLS tests cover actual certificate replacement/reuse. Immutable certificate history is never modified.

Mutation bodies have a total 10-second read deadline (`--body-timeout-secs` /
`PORCHLIGHT_BODY_TIMEOUT_SECS`): 408 `request-timeout` closes the connection. GET/SSE are unaffected.
Expired presented certificates return 401 `device-certificate-expired`, including keep-alive requests and
renewal; affected devices must re-enroll, and open streams close at their next heartbeat.
Startup waits for the revocation watcher and stream hub before accepting HTTP. Expired sealed idempotency
responses are purged at startup and hourly. The gate pins a CA in an isolated curl wrapper and checks that
a secret-free LAN request using a wrong CA fails verification.

The dev server SANs include `localhost`, `127.0.0.1`, emulator host alias `10.0.2.2`, and the runtime LAN
IPv4 address. `scripts/dev-stack.sh` and the M2 demo choose the lowest-metric physical interface from
`ip -4 -o route show table main default` (missing metric = 0; ties keep the first listed), using its `src`
or primary global IPv4 address and matching subnet from `ip -4 -o addr show scope global`. Physical means
`/sys/class/net/<interface>/device` exists (`DEV_LAN_SYSFS_ROOT` overrides `/sys` for offline tests).
Only without a physical default does discovery fall back to `ip -4 -o route get 1.1.1.1`; nonphysical
`/32` tunnel results fail clearly. Discovery sends no network traffic. Override with `DEV_LAN_IP` /
`DEV_LAN_CIDR` (for example `192.0.2.10` / `192.0.2.0/24`) to select another LAN.
Missing/invalid discovery fails clearly before stack startup. Changes to the LAN address require renewing the dev server SANs or resetting dev state.
Enrollment advertises `localhost` by default; the native `dev-init` helper can override its TCP destination
while retaining the URI's TLS identity. For actual emulator enrollment, set `ENROLL_HOST=10.0.2.2` in
`var/dev.env` and restart `up`. Dev-stack enrollment admits the runtime LAN CIDR plus Docker's backend/ingress
gateways and advertises local, emulator and LAN endpoints; direct Compose defaults are local/emulator-only.
**Production must list only the LAN CIDR**, never these dev gateway exceptions.
With the dev CIDRs, **every local process and every container on the laptop** passes the enrollment peer check
via Docker's gateways. The gate observed a container on an unrelated network arriving as the backend gateway.
This exception is for dev/emulator convenience, not a production LAN-only boundary.

```bash
set -o pipefail
mkdir -p docs/operations/gates
deploy/scripts/m2-demo.sh 2>&1 | tee docs/operations/gates/m2.log
scripts/ci.sh --full  # M1 against dev stack, then isolated M2; writes only var/gates/m2-ci.log
# After a human-attested laptop reboot, with existing dev desktop credentials (do not start the stack first):
deploy/scripts/m2-demo.sh --after-reboot 2>&1 | tee docs/operations/gates/m2-reboot.log
```

The M2 gate uses project `porchlight-m2demo`, fresh `var/m2-demo`, ports 15432/18443/18444/19445, independent
Postgres volume and networks, and a container-side init using **only the demo database**. It tears down on exit;
`--keep` leaves it up for inspection (the next run still starts fresh). Public PASS/FAIL output and source-IP
findings are safe for the gate log; private diagnostics/raw responses remain under `var/m2-demo`.
`--after-reboot` only checks the running dev stack; it never starts it or proves a reboot happened.

The Compose file supports bind/port/network overrides (`PORCHLIGHT_VAR`, `PG_PORT`, `LAN_PORT`, `ENROLL_PORT`,
`SIDECAR_PORT`, `BACKEND_SUBNET`, `INGRESS_SUBNET`, `BACKEND_GATEWAY`, `INGRESS_GATEWAY`, `SIDECAR_IP`,
`ENROLL_CIDRS`, `ENROLL_HOST`, `PORCHLIGHT_ENDPOINTS`) and project selection with `-p`. Override the network
gateways and dev-stack's runtime enrollment CIDRs follow them; if setting `ENROLL_CIDRS` explicitly, update it alongside
the gateways. The dev helper exports the actual host UID/GID; direct Compose defaults to
1000:1000, so export `PORCHLIGHT_UID=$(id -u) PORCHLIGHT_GID=$(id -g)` if invoking Compose yourself.

## Live stream (S6b / M5)

The member-session stream routes every supported tickle by current item/list/member scope, rechecks
queued replay ACLs, evicts former readers, and requests a snapshot for expired/pruned cursors.
Heartbeat-held sessions slide hourly; shutdown sends `drain` then clean EOF. Full wire catalogue and
seven-day replay boundary: [stream.md](docs/specs/stream.md).

Controller-only TLS gate (two distinct dev members, credentials under `var/`, SAME privileged dev
DB as the TLS API; configure the `M5_STREAM_*` variables privately as documented in the script):

```bash
deploy/scripts/m5-demo-stream.sh  # S6a's m5-demo.sh can chain this; PASS/FAIL only
```

Never use production: the gate creates/soft-deletes a fixture item and temporarily seeds aged
outbox events. Private responses/diagnostics stay in `var/m5-stream-*/`.

## HTTP contract (S1 mutation protocol)

Every HTTP request, including enrollment, the agent socket, unknown paths and SSE, sends
`Porchlight-Client-Version: MAJOR.MINOR.PATCH`. The server minimum is configured by
`PORCHLIGHT_MIN_CLIENT_VERSION` / `--min-client-version` (default `0.1.0`); below it returns 426 with
`min_client_version`. `porch` sends its own compiled version. `Porchlight-TZ` is optional, but must be a valid IANA
zone when present; reporting it does not change time-zone authority. Non-GET requests require `Idempotency-Key`.
Versioned domain updates/deletes use `If-Match: <version>` (or a quoted integer); missing is 428, stale is 409 with
`current`. M2 trust operations are not versioned domain updates.

Device collections accept `?limit=50&cursor=…` (maximum 200), returning `items` and `next_cursor` while retaining the
legacy `devices` field. Cursors are opaque signed key tuples, bound to the collection/household/filter. Problems
are listed in [the generated catalog](docs/specs/problems.md); transaction ordering is in
[lock-order.md](docs/specs/lock-order.md).

Regenerate the contract after annotating routes:

```bash
. scripts/env.sh
(cd server && CARGO_NET_OFFLINE=true cargo run -q -p porchlight-api --bin porchlight -- openapi) > docs/openapi.json
```

New API areas expose `routes()` and a derived `AreaApi`, merge once in `routes/mod.rs` and `openapi.rs`, and add
entries to the single route registry. Register mutations with `mutation::{post,put,patch,delete}` and run domain
writes through `mutation::run`, passing `Caller`, the parsed `Mutation<T>`, and a callback returning a response.
The callback receives `(caller, &mut IdemTx, request)`; `tx.tx` is the actor-attributed SQL transaction. Business
errors are stored; 500s roll back the claim and all effects. Agents alone `sees_all`; app super admins still follow
visibility. Session-less login/reset and enrollment use specialized authentication adapters and the same
`mutation::execute` sealed-response/commit lifecycle. Revocation publication happens only after commit.
Startup purge failures log and retry hourly instead of preventing serving.

## Item and administration CLI (S5)

All commands use the agent socket and support `--json`. Credentials come from private token files or
`PORCHLIGHT_TOKEN`, never an argv token. Mutations auto-read versions unless `--version` is supplied.

```bash
porch items add --title "Dentist" --start 2026-11-01T09:00:00 --end 2026-11-01T10:00:00 --tz Europe/London --assign admin --json
porch items list --json
porch items edit ITEM --title Updated --json
porch items take ITEM --json
porch occurrences list ITEM --json
porch occurrences complete OCCURRENCE --json
porch agenda --from 2026-11-01T00:00:00Z --to 2026-11-08T00:00:00Z --json
porch board --json
porch search Dentist --json
porch members list --json
porch users list --json
porch users add --display-name Child --role dependent --json
porch users edit MEMBER --display-name Updated --json
porch users rm MEMBER --json
porch devices list --member admin --json
porch revisions ITEM --json
porch revert ITEM --to REVISION --json
porch purge --item ITEM --yes --json
porch purge --deleted-before 2026-10-01T00:00:00Z --yes --json
porch derive status --json
porch derive retry ITEM --json
```

Users and purge require ADMIN-scoped PATs; revert/retry require WRITE or ADMIN; revisions/status accept READ.
Purge accepts at most 200 soft-deleted items and records immutable actor/item/count audit. Without `--yes`,
purge requires a terminal confirmation and fails closed in noninteractive mode. Items support local or RFC3339
`--start/--end/--due`, `--floating`, `--all-day`, `--date`, `--every`, `--rrule`, `--missed`, `--clear`,
and `--scope this|all|this-and-future` with `--occurrence` for recurring edits.

S5 CLI time/administration notes: `--start` infers an appointment and requires `--end`
(no implicit duration); `--due` infers a deadline. Fixed timed items need `--tz` even
with RFC3339 instants; floating items store local anchors, and fixed all-day bounds
are resolved in `--tz`. `--every` supports m/h/d/w/mo/y; `--missed` accepts
carry-over, skip or stack. New users default to the household zone; dependents use
it (a different explicit `--tz` is rejected). `derive retry` reports `queued` only
for an existing pending generation, not merely because a retry was requested.

CLI whole-response JSON goldens live in `server/cli/tests/goldens/`; UUID linkage
and response shapes are retained, generated IDs/timestamps are normalized. To
intentionally refresh them, source `scripts/env.sh` and run
`PORCH_UPDATE_GOLDENS=1 CARGO_NET_OFFLINE=true cargo test --manifest-path server/Cargo.toml -p porchlight-cli --test domain --locked`.
Normal CI compares only and never updates snapshots.


### Notifications (M5 server)

`porch notifications list --unread --json`, `inspect --member <uuid> --json` (admin PAT), `read <uuid>`,
`read-all`, `prefs --json`, `set-prefs --file <preferences.json> --version <observed-version>`.
Canonical preferences, source ACLs, stream tickles and controller demo setup: `docs/specs/notifications.md`.
Server gate: `deploy/scripts/m5-demo.sh --sandbox`; provisioned dev stack: `deploy/scripts/m5-demo.sh --stack
var/m5-demo/config.json` (private runtime config; no secret output). The stack config requires two devices for the recipient and one for another member; this gate checks cross-device read/read-all dismissal and automatically chains S6b’s full stream checks.

## Comments and attachments (S7b / M6)

```bash
# Agent credentials stay in a private file under var/; never put the PAT in command arguments.
porch --socket var/run/agent.sock --token-file var/bootstrap.pat --json comments add <item-uuid> 'Remember @Alice'
porch --socket var/run/agent.sock --token-file var/bootstrap.pat --json comments list <item-uuid> --limit 50
porch --socket var/run/agent.sock --token-file var/bootstrap.pat --json attach <item-uuid> var/notes.txt
```

Comments inherit item visibility, are plain text (10000 characters), and are author-editable through 15 minutes
inclusive. Mentions match live display names case-insensitively at word boundaries, longest first (`@Alice`, `@Alice Smith`); unknown/ambiguous @tokens remain plain text. Authors or visible super admins may soft-delete; PATCH/DELETE require the comment version in `If-Match`.
Attachment uploads accept one multipart `file` field, capped at 25 MiB while streaming to temporary disk, and use the bounded K6
sniff/decode/strip pipeline. Images, PDF and text are supported. WebP main meta-Huffman streams are deliberately
unsupported (415); opaque VP8 may be stored as JPEG, so download extensions follow the stored MIME, not the upload
name. Downloads and thumbnails use attachment IDs, never hashes, and return `nosniff` with sanitized filenames.
Creating comment attachments is an author edit within the same 15-minute window (even for super admins).
Deleting a comment attachment requires its comment author within that window or a visible super admin for moderation;
both operations bump the comment version. Item attachments remain deletable by uploader or item editors. DELETE requires `If-Match`.
Upload bodies have a 30-second idle timeout and 10-minute overall cap. Rate limits run before streaming, with at most
two temp uploads per device/PAT (429 `rate-limited`, `Retry-After: 1`); decode admission follows the complete upload and precedes the mutation DB transaction. Completed idempotent replays bypass decoder admission (still authenticated/revalidated). Unicode filename categories Cc/Cf/Zl/Zp are stripped using generated Unicode 16.0.0 data.
`PORCHLIGHT_DATA_DIR` / `--data-dir` configures the
server data volume (CLI default `/data`); local direct runs must point it under `var/`. Daily 30-day GC respects
backup leases.

Comments notify the owner/assignees and newly mentioned live readers, excluding the actor and hidden readers.
The repeatable-read snapshot includes comments and attachment metadata; search unions visible items, lists/entries and comment bodies
with a signed source/id cursor. Backups commit a blob lease before their snapshot, renew it on byte/dump progress
and immediately before publication, then release it after successful publication or failed cleanup.
The controller TLS/socket gate is `deploy/scripts/m6-demo-collab.sh`;
its input variables identify an existing item, enrolled device directory and private PAT file under `var/`.

### Data export and backup health (O2 server)

```sh
porch export --member MEMBER --out var/member-export.json
porch export --household --out var/household-export.tar.gz
porch backups --json
```

Exports require an admin PAT for CLI selection; member data follows the selected member's ACL, never agent
visibility. Household v2 archives contain a data.json manifest, tables/*.jsonl, referenced attachment/thumbnail blobs
and a LAST complete.json inventory. An archive without that completion entry or a valid gzip trailer is
incomplete; porch verifies both before reporting success. They exclude credentials,
sessions, PATs, feed tokens, PKI or configuration secrets. Output files are private and never overwritten;
omit --out for raw stdout. App household export additionally requires a single-use `household.export` step-up.
Super-admin mTLS apps can read `/v1/backups`, `/v1/backups/latest` and range-resume the encrypted
`/v1/backups/latest/download` using its SHA-256 ETag. `PORCHLIGHT_BACKUP_DIR` / `--backup-dir` defaults to
`/backups` (local runs must use var/). `/v1/admin/backups` is the admin-PAT status surface, not a download.
Nightly retention and hourly backup-stale alerts are documented in [data-out contracts](docs/specs/data-out.md).
Super-admin desktops pull nightly into `~/.local/share/porchlight/backups`, resume encrypted checkpoints,
verify SHA-256 and keep the newest eight per household. Settings → Account shows backup status (DESIGN-PENDING).
See [passive pull and pulled-bundle restore](docs/operations/passive-pull.md), [exports](docs/operations/export.md),
[retention](docs/operations/retention.md) and [M8 acceptance](docs/operations/m8-acceptance.md).

```sh
python3 -I deploy/lib/verify-export.py var/household-export.tar.gz
# Controller only; substitute two actual increasing signed release versions.
deploy/scripts/m8-acceptance.sh --source dist/0.1.0 --upgrade-source dist/0.1.1 \
  --identity var/backup-recovery.key --keep
```


## Client calendar data and planning (C6)

Settings → CalendarData/Availability/Planning/Export adds ICS preview/commit, subscribed-feed administration, weekly windows, EDF accept/dismiss and dependent candidates, and verified export downloads. Super-admin Settings → Devices uses password-confirmed session revocation; own logout remains Account. Calendar shows availability and suggested-time cards. Server contract currently lacks dated availability exceptions, independent load limits and device expiry fields; screens explain these omissions. See `docs/operations/client-planning.md` for interfaces, file integrity, acceptance and exact controller checks.
