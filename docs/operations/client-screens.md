# Calendar, household settings, local resume and M4 gate (C3b)

## Interfaces

- Backward-compatible serialized `AppRoute.Calendar(view,date,memberId,overlay)` drives desktop Day/Week/Month and mobile Day/Week/Agenda. Prev/Next/Today/view changes replace the Calendar entry and persist its context; Android restored Month is displayed as Agenda until the design pass. Item links use the existing Item route (C3a). Calendar selections use the member's cached server `tzid`, not the device's zone; GET agenda resolves floating times server-side. Primary member selection and optional comma-separated overlay IDs live in the route and survive navigation/resume; only cached valid members are used. Primary selection controls the grid zone, while all displayed rows remain server-resolved instants.
- `calendarWindow(view,date,zone)` returns half-open instant boundaries and dates, Sunday weeks and complete month grids. `calendarDays` only positions the selected member's server agenda rows; it splits midnight-spanning intervals, retains 23/25-hour DST days, date-only/all-day rows, conflicts and pending flags. `calendarGrid(window,rows,memberIds,zone)` adds deterministic elapsed-minute offsets/durations, connected-overlap column allocation, UTC-offset DST ticks and cross-day all-day lanes for the desktop grid; `calendarDays` is the single-member compatibility seam. No client recurrence/floating-time derivation.
- `AppServices.settingsWrites` owns `HouseholdSettingsController` for the application lifetime. Production Android/Desktop services create it lazily from `data`. Screen bodies use generated `SettingsRead`, `TodaySettings`, `MemberRead` and `MemberPatch`. It preserves unknown-to-UI Today fields (including pinned ID; conflict reapply uses the latest server pinned ID), uses expected version and original idempotency key, requires explicit conflict reapply/discard, and refuses a new write while an ambiguous operation is unresolved. Cancellation/relock/transport failures retain the exact operation for an intentional same-key replay; success with refreshPending remains success. Identity epoch changes discard prior-identity replay state, including late errors. Server `errors:[{field,message}]` are currently shown in the recovery summary (inline field attachment remains DESIGN-PENDING); non-ambiguous Error responses can be edited/discarded rather than replayed. Saves still use the composition scope: leaving Settings cancels into Ambiguous, which must be explicitly replayed after returning.
- `TodayDraft` preserves section order, toggles disabled sections without losing their slots, validates 1..14 days/scope and refuses an enabled pinned section without a list ID. C5 owns selecting/validating pinned lists.
- `AppServices.routeAvailable` calls only `ReadCache.initialize()` (disk) and `localRouteAvailable` (immutable cached rows). Authoritative item rows take precedence over projections; deleted/non-view-capable/missing items fail closed. Existing cached Board/Agenda items support projection-before-snapshot resume. Selected lists fail closed until C5 adds cached list ACL/existence. Cache open/decode failures fail closed to Today; cancellation is preserved. No generated request or credential access precedes trust/PIN gating.
- Shared `ShellButton` adds a default-true `enabled` parameter so unresolved saves/unsupported pinned selection have real disabled semantics. Shared navigation/AppServices changes are additive; Today/Board/Item placeholders remain for the concurrently implemented C3a.

## Designs and accessibility

Desktop Day/Week uses the DesktopCalendar hour grid, multi-member overlay, shared server member colours, overlap columns and all-day/multi-day lanes. Transition days have 23/25 elapsed hours and disambiguated repeated wall hours. Grid row height scales with font size; short entries keep complete accessible descriptions while omitting subtitles, and point/due-only entries reserve an hour without inventing an end label. Month remains a DESIGN-PENDING seven-column card grid. Mobile Day/Week uses stacked cards; Agenda omits empty days, and Month is deferred. All-day rows are separate; fixed/floating labels, pending status and Busy conflict warnings are textual, never color-only. Member chips include initials. All controls have descriptions and >=48dp targets; forms wrap/scroll at 200%, use explicit saves, and support up/down reorder alternatives. Notification preference content remains C4's placeholder; Account continues to use C1b's gate-safe panel. See design-pending.md for remaining visual design review.

## Verification and controller commands

Fast sandbox CI includes host model/JVM Compose tests, both APK compilations, lint and deterministic netsimd process-seam tests. Instrumented C3b tests are ordinary managed-device tests, never `@ControllerOnly`; that exclusion rule is unchanged. JVM tests use both themes and 200% font scale, actual text line extents (1px rounding tolerance; Compose's container-width overflow flag is misleading for intrinsically sized text), date navigation, Busy/all-day labels, initial chips, explicit conflict/replay actions and cold local-only resume. All clocks/dates are fixed/injected; no UI-thread runBlocking or sleeps.

Run on the laptop controller (not an implementer sandbox):

```bash
umask 022
. scripts/env.sh
scripts/test-managed-clients.sh
# Exact underlying suite, if diagnosing; prefer wrapper for stale-daemon cleanup:
# cd client && ./gradlew --offline --console=plain --max-workers=4 :androidApp:verifyManagedDeviceFilter :androidApp:phonesGroupDebugAndroidTest
scripts/smoke-clients.sh porch-api35
deploy/scripts/m4-gate.sh
scripts/ci.sh --full
mkdir -p docs/operations/gates
cp var/gates/m4-ci.log docs/operations/gates/m4-ci.log
```

The managed-device wrapper reaps this user's stale netsimd before Gradle can boot API35/API36 and after completion/failure. The smoke script does the same around its owned AVD's boot/shutdown, preserves running emulators/other AVDs, packages desktop + APK, installs/launches on porch-api35 and launches the packaged desktop. A daemon ignoring TERM is escalated to KILL only if no emulator is alive. Do not run booting tools concurrently; active-emulator safety deliberately skips cleanup.

Live acceptance still requires an enrolled household: server Today/color changes appear on another device after refresh; stale settings show compare and explicit reapply; PIN relock mid-write permits identical replay; calendar floating rows follow member zone across DST and hidden conflict titles remain Busy; saved deleted/invisible item falls back to Today before shell composition. Native/emulator/packaged launch results are controller evidence, not proven by compilation.

Calendar entry navigation (agenda and desktop grids) carries the exact occurrence ID. Item completion, this-occurrence edits and this-and-future splits resolve that occurrence from occurrence/Board/Agenda cache and retain its version; an explicitly unavailable occurrence never silently falls back to the next Board occurrence.
