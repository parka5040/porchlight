# C6 calendar data and planning clients

Shared Android/desktop Settings sections: CalendarData (ICS upload and super-admin feed tab), Availability,
Planning, Export, Devices (super admin). Calendar Day/Week adds member-local availability bands plus EDF proposal
cards and links to the full planning/availability forms. Settings → Account remains C1b's own-device logout panel.
No new AppRoute or server route is introduced; Settings section enums extend persisted navigation additively.

## Binding interfaces for C4 / later clients

`AppServices.data` adds `imports`, `feeds`, `availability`, `planning`, `exports`, `deviceAdmin`.
All JSON responses are generated models from the canonical OpenAPI (including R65 additive read fields). Repositories accept explicit `key`
parameters for mutations, use `WriteCoordinator` (same-key transient contention; authorized current on stale version),
`GeneratedCall` trust gates and `cache.inSession` leases. `AppServices.stepUp(password, action)` uses the existing
C1b controller; action names are `device.revoke` and `household.export`. Never use PAT admin-device operations for an
app session: app GET /v1/devices returns all members' devices to a super admin, and POST /v1/devices/{id}/revoke consumes
step-up. Current-device revocation is deliberately delegated to Account, which clears local trust correctly.

- `imports.preview(ImportRequest)` sends preview=true; `commit(request,key)` changes only preview=false. Summaries
  retain server skipped/warning reason codes and UID labels. Commit success queues snapshot refresh; failed follow-up
  reports refreshPending, never resubmits. Cancel drops the preview; no server-side committed data exists to undo.
- `feeds.list/add/update/refresh/remove` use session calendar routes. Update/refresh/remove send Source.version If-Match. Feed UI
  uses server error detail (including SSRF rejection), status/errors, and confirmation of soft removal/history.
  Create/edit supports household/member/dependent target, private/household visibility, polling enabled and 60–2592000-second interval. Changing the target defaults household/dependent targets to household visibility and ordinary member targets to private; the user can override it. Refresh now reports queued (202), never completed; rows expose URL, enabled, failure count and next-fetch/backoff. Successful creation clears the form; re-adding a removed URL creates a new source.
- `availability.rows/observe(member)/refresh(member)/put(member,version,windows,key)` persist typed weekly collections.
  Monday=0, equal endpoints empty, end<start overnight, overlaps unioned server-side. HH:mm is normalized to whole
  seconds. Empty availability means load untracked. UI permits self, and other members to super admins; server remains
  authoritative. Calendar shading resolves each endpoint exactly like the server: DST folds use the earlier instant;
  gaps snap to the first valid wall-clock minute (not a shift by the gap duration). For example New York's
  2026-03-08 02:30–03:30 shades 03:00–03:30. Snapshot/receipt versions prevent an older in-flight snapshot overwriting a newer acknowledged PUT.
- `planning.rows/refresh(member)/suggest(member,key)/accept(row,key)/dismiss(row,key)/candidates(item)` persist typed
  suggestions and expose pending candidate derivation. Accept uses occurrenceVersion, dismiss suggestion.version.
  On-demand UI runs for self or (super admin) a dependent, not other ordinary accounts. Unplaced/candidate reasons
  are rendered as human-readable text; member names and locale-aware member-zone dates (with offset for DST folds) accompany EDF cards. EDF cards explain the algorithm (there is no per-proposal reason field). Accept/dismiss/availability
  writes queue full post-write snapshots to honor R59a; refreshPending means reread, not repeat. Sealed 422 accept/dismiss refetches the member; both planning panels expose manual refetch. If the server still lists an accept-rejected proposed row (e.g. not-future), session-owned id/version/occurrence-version rejection stamps keep Accept disabled across refetch/snapshot/PIN relock; Dismiss remains available when not busy. A new proposal/version is actionable and identity change clears the stamps. Only proposal acceptance uses the expiry message override; other 422s retain their server detail.
- `ReadCache` adds availability/suggestions to CacheView; snapshot publication is atomic with the existing entities
  and cursor. Suggestion eviction follows item ACL eviction. Identity clear deletes planning rows. SQLDelight schema
  version increases to 2; the disposable disk cache is recreated, device settings/trust remain intact.
- C4: tickles for availability/suggestions should call the appropriate refresh or `sync.refresh()` for complete ACL/
  derived reconciliation. Source tickles call feeds.list only while a super-admin feed panel is active; no offline
  feed URLs/device lists are retained. Ordinary member/item ACL changes must still take C2b's full snapshot path.
- UI writes retain immutable request/key in application `ScreenSession`; `planningDrafts` survives PIN relock and
  Activity recreation, and clears with identity epoch. No offline write queue. Availability stale-version UI shows
  current/yours, with explicit current-version choice or refetch/discard. Other stale mutations require refetch/new
  action. No automatic overwrite/replay of ambiguous requests.
- Availability windows stay editable while a save is in flight. The draft counts local edits; the save reply (applied
  inside the application-scoped write, so also off-screen) always advances the version but replaces the windows only
  if nothing was edited since that save was sent. Otherwise the newer edits stay and the form says "Not saved yet"
  until a later save sends them against the new version.

## File I/O and export integrity

Android uses ACTION_OPEN_DOCUMENT / ACTION_CREATE_DOCUMENT via a private translucent helper; no broad storage
permission. Only the selected URI is read/written. Desktop uses a Swing chooser on EDT with I/O off-main. ICS requires
UTF-8, .ics filename (or Android text/calendar MIME) and ≤5 MiB. Each file uses a normalized filename scope, bounded to 200 characters (long names use a truncated prefix plus stable 64-bit FNV-1a suffix); preview failures can be retried without re-picking. Same-frame duplicate import taps do not launch another picker. Import commit clears the feed-refresh queued status, so its successful result reports Saved. Desktop refuses overwriting an existing filename.

Desktop ICS reads consume at most 5 MiB + one overflow-detection byte, independent of reported file size or
concurrent growth. Desktop export destinations are created atomically with POSIX 0600 permissions before any
plaintext is copied where the filesystem supports it. After writing, the effective permissions are re-read; on
locations that ignore the request (vfat/exFAT USB sticks, other non-POSIX mounts) or whose privacy cannot be
confirmed, the export is kept and reported as saved with a warning ("this location can't keep the file private;
others with access to this drive may read it"). It does not fail. Existing files (including symlinks) are never
overwritten or chmodded. Android's selected-URI/SAF behavior is unchanged.

`ApiTransport.download` and `GeneratedCall.download` add bounded 64KiB streaming through C1b's same pinned mTLS HTTP
client, existing session/version/zone/step-up policy, connect-only failover and no ambiguous replay. Each body read
has a five-minute idle deadline. `ExportSink` stages to a private file; `finish(checkGate)` validates then checks the
identity/PIN gate before publishing and every copy chunk. Any failure/relock/cancellation calls abort under
NonCancellable; only a destination successfully opened for publication is removed where the provider supports delete. A pre-publish/network/validation/step-up failure never deletes an untouched confirmed-overwrite document.
A revoked certificate from an obsolete identity cannot clear a newer identity.

Member export is JSON with tables plus appointments_ics; validation requires format_version=1/tables/ICS and imposes
a 64MiB client JSON-document cap (explicit limitation, not a server cap). Household tar.gz validation is streaming:
complete.json must be the final entry, format2/complete=true, blob-count/hash inventory and table names must agree,
and gzip CRC/trailer must validate. Nothing is extracted/executed. Crashed private spool files may require manual
cleanup in the app cache/data exports directory; successful/failed/cancelled operations delete their own spool.

## Server contract gaps (not invented client fields)

The current availability API has no dated exceptions or independent load-limit fields/routes. Capacity is computed
from weekly windows; UI explains this. Dated exceptions and independent limits are brief overreach, not missing spec requirements. R65 now exposes Device.certificate_expires_at on session/admin reads (latest-issued certificate; null for revoked/no certificate), and ImportSummary.invalid_skipped/cancelled_skipped while retaining legacy skipped. Parser-only InvalidEvent warnings retain their pre-existing count behavior and are always displayed with UID/reason.

## Tests and controller gates

Fast/sandbox CI includes common repository/version/key tests, snapshot/receipt atomicity, member-zone overnight/DST
bands, Ktor binary streaming/header and relock abort tests, real Rust-handler recording equality, JVM generated-model
fixture decode, connected import ambiguous-retry/removal/recomposition, archive completion/trailer and both-theme
200% named 48dp screen semantics including populated actions and ordinary-member role gates. Instrumented APK includes equivalent six-screen semantics plus production SAF
intent/helper/parser with a deterministic mocked system-activity result and a debug-only document provider, plus pre-publish overwrite preservation. The instrumented density uses the real display density at 200% font scale.
The mock intercepts only the SAF action (an `ActivityMonitor` overriding `onStartActivity`): an `IntentFilter(action)` monitor
also matches the action-less explicit `CalendarDocumentActivity` launch and would swallow the helper instead of the dialog.
No emulator or real desktop dialog was executed in the implementer sandbox.

Controller, merged tree (source scripts/env.sh and keep max-workers=4):

```sh
(cd client && ./gradlew --offline --console=plain --max-workers=4 :shared:jvmTest :shared:testAndroidHostTest :contract-check:jvmTest :androidApp:assembleDebug :androidApp:assembleDebugAndroidTest :desktopApp:jar :androidApp:lintDebug :shared:lintAndroidMain)
scripts/ci.sh --sandbox
scripts/test-managed-clients.sh
# Keep wrapper's stale netsimd cleanup and ControllerOnly exclusion; do not boot manually without cleanup.
deploy/scripts/m7-demo-ics.sh
deploy/scripts/m7-demo-planning.sh
scripts/ci.sh --full
```

Live manual checks on both AOSP APIs and desktop: enroll/PIN unlock → ICS pick preview/cancel/commit/re-import;
feed HTTPS create/SSRF-detail/refresh/error/remove/read-only item; own/dependent availability and EDF accept/dismiss/
stale versions, candidates and pending derivation; offline cached availability/suggestions, relock identical-retry;
member save and household password step-up save, inspect format/complete marker; super-admin per-member device
list/revoke another device and Account own logout. Cancel choosers; rotate Android helper; real SAF grant/save/delete
provider behavior; fonts 200%/both themes; visually review member bands/DST and planning cards against DesktopCalendar.

To intentionally update recordings after an authorized wire change (not normally needed):
`PORCHLIGHT_RECORD_PLANNING_FIXTURES=1 CARGO_NET_OFFLINE=true cargo test --manifest-path server/Cargo.toml -p porchlight-api --features testing --locked --test planning client_planning_fixtures::`, then run without the flag and JVM fixture tests.

## Picker/PIN relock

Platform dialog I/O uses the identity-owned ScreenSession scope. A document picker can STOP the main Activity and relock its PIN; the returned file is held only in the memory draft, and preview starts from a gated LaunchedEffect after unlock. Import/export drafts are keyed by section and the signed-in member id, not the availability/planning member selector. Save callbacks retain the staged sink in that identity-owned draft and await a gated LaunchedEffect: neither member nor household download starts until unlock. A returned export destination waits at most 60 seconds for gated initiation; timeout/cancellation fences the pending request so an old step-up token cannot start later. Entering another planning section cancels noncurrent pending export drafts, including obsolete member-keyed drafts. Active downloads are not limited by this unlock deadline, but navigating away cancels them with the neutral message “Export interrupted before completion. Try again.” Pending-picker/export actions are disabled; helper-owned slots reject duplicate launches and clear lost helpers without cancelling newer requests. Application context prevents Activity capture across rotation. Timeouts/cancelled UI downloads report failure while the same identity scope is active; an ended identity remains silent and cannot repopulate UI. C6PickerRelockTest proves import and member/household save disposal/result/unlock ordering and duplicate-tap disabling; real Android picker/credential lifecycle still needs controller verification.

Focused regressions: `C6AdminPickerRelockTest` covers selected-dependent section switches, import and member/household
export relock/unlock with exactly one request; `PlanningFix2Test` covers orphan cancellation, unlock deadlines, scope
stability and feed defaults; `C6Fix2BehaviorTest` covers same-frame taps, Saved feedback, unrelated 422 detail and
expired-card dismissal. Real DocumentsUI/JFileChooser lifecycle remains a controller gate above.
