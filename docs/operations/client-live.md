# C4 live client checkpoint

The sign-off requirements below describe the historical laptop-dev gate; the owner decision in the
[M5 section](#m5-client-demo-and-fresh-avd-soak) supersedes them for final acceptance.

R64 background/PIN, durable delivery, fake-bus desktop protocol coverage and controller gate tooling are implemented. **M5 is not signed off until the controller runs the real device/session-bus demo and 30-minute fresh-AVD soak.**

## Interfaces

- `AppServices.data.live`: application-owned `LiveSync?`, initialized by `HouseholdData.bindTrust`; `state`, `unreachableSince`, `networkChanged()` and `successfulRequest()` are reachability/reconnect seams. Ready and PinRequired share a usable background session: relock neither cancels nor reconnects SSE. Session replacement reconnects even when expiry/re-login state changes conflate. Epoch/token checks still fence frames, problems, generated replies and cache commits.
- `withBackgroundSession(controller)`: application-owned scope captures the original epoch/token. Only sync reads and notification Complete/Read routes can use the stored key/session while PIN-locked. UI requests still throw HouseholdLocked/HouseholdLockedAfterReply; account/device/admin/step-up calls require the PIN. All other trust gates block background API work. TrustGate still hides cached UI content. Workers restore the key/session without unlocking the UI.
- `NotificationRepository`: typed recipient-private `notifications`/`prefs`; paginated refresh, Read/ReadAll and PutPrefs. ScreenWrites retains immutable UI retry keys/body/version. Six channels remain unchanged. Device-local Stay connected is opt-in.
- `ReadCache.acknowledgeCursor(seq)`: durable monotonic acknowledgement AFTER refetch/eviction. Cache schema 2 stages notifications/prefs with the atomic snapshot. `putOccurrences(...,includeUndated)` removes only rows authoritatively covered by the answer.
- `AlarmLedger`: private durable codes/generations, seven-day reconciliation and pending-derivation preservation. Inactive/delivered tickets older than seven days are pruned; monotonic nextCode prevents reuse. Android caches the ledger per process and commits only changed preference rows; desktop checkpoints changed per-code files under `alarm-tickets/`. Legacy tables migrate once. Delivered tickets also reconcile remote completion/eviction. Codes are not reset on identity end. Snooze uses original code/generation/timestamp and only due/delivered tickets; retry returns the identical ticket, older generations never overwrite a newer snooze. Source fireAt is retained, so server reschedules override local snooze; delivered snooze clears its flag. Today offers snooze only after delivery.
- `DeliveryInventory`: separate from disposable read cache, persists OS tags/versions/handles, pending delivery and local-dismissal tombstones (no titles/bodies/session tokens). Android commits before notify and after success; replay replaces the same tag. Cold-start unread alerts are delivered; remote read/removal/completion cancels persisted OS deliveries, including local alarms. Permission-denied local delivery remains pending and is retried on recovery. Desktop additionally persists the notification server's unique bus owner: restore handles only for that owner, never cancel reused IDs on a replacement server. Desktop delivery is at-least-once across the unavoidable Notify-reply-to-disk crash window, not an exactly-once claim.
- Android notification receivers enqueue expedited WorkManager (including snooze), never perform network writes. Durable inputs include original id/version/key and a non-secret session fingerprint; old-account work cannot acquire replacement credentials. Complete/Read use the scoped repositories. Version conflicts/terminal errors produce a persisted nonsecret recovery notice, not silent automatic conflict replay. Pre-31 expedited fallback has ForegroundInfo; WorkManager and stream services declare specialUse. Due alarms read disk only and do not require network/PIN; posting inventories are committed before the alarm is marked fired.
- Android background stream service is user-opt-in and START_STICKY; BOOT_COMPLETED restarts it only when opted in. Foreground UI also enables SSE for its lifetime. An opted-out background receiver performs disk-only delivery/reschedule, never a bind-triggered snapshot/SSE; its 15-minute worker performs one snapshot. Opted-in workers reuse live binding/replay. Application-owned NetworkCallback wakes reconnect. Receiver scheduling and durable ledger writes run on IO/goAsync. Exact alarms use USE_EXACT_ALARM on API33+ and SCHEDULE_EXACT_ALARM maxSdkVersion=32 (R66); only API31-32 supports normal denial/recovery testing.
- Desktop `DesktopBus` exports SNI/dbusmenu and uses freedesktop Notifications. Alarm timers wait for the next fireAt, bus owner/watcher changes use signals, and reachability waits for lastSuccess+5 minutes. No healthy-bus polling or unchanged ledger writes. Session-bus absence retries at 15-second intervals. `DesktopInstance` uses a private lock and WatchService focus signal. Corrupt delivery JSON gives a notice and rebuild, never an indefinitely pending initializer.
- Desktop **Start at login** is opt-in and resolves the packaged `bin/porchlight` wrapper from the native launcher (`jpackage.app-path`/process command), using the installer's stable current symlink when available. The app owns one XDG entry; the installer never enables XDG startup. Explicit installer `--niri` selects the alternative spawn-at-startup mechanism and removes XDG startup only after activation. Settings shows a selectable niri snippet; leave Start at login OFF when using it. Runtime refuses a duplicate XDG entry if niri already starts Porchlight. Existing `--minimized` niri entries remain supported; new entries use `--tray`. No systemd units.

For black/unresized desktop content on niri/xwayland-satellite, the packaged launcher now automatically sets
Java's non-reparenting-WM flag on Wayland except known reparenting desktops (GNOME, KDE, Cinnamon/X-Cinnamon,
Pantheon, Unity). No manual niri environment setting is needed: `PORCHLIGHT_AWT_NONREPARENTING=0|1`
disables/forces it, while an existing `_JAVA_AWT_WM_NONREPARENTING` wins. X11 keeps Java's own detection.

- `AppServices.pendingOpen`: OS taps request a route; shell rechecks availability behind the trust/PIN gate. `runtimeNotice` carries redacted platform/permission/action degradation notices.

## Desktop protocol and ONE tray

`DesktopBus` exports one SNI and one dbusmenu. Status becomes NeedsAttention for five-minute unreachability or unread alerts; title/icon/tooltip and PropertiesChanged/NewStatus signals reflect changes. dbusmenu supports parent/depth/property filtering, GetProperty/GetGroupProperties, Event/EventGroup, AboutToShow/Group, typed nested variants and layout revisions. A fake session bus exports these exact objects through dbus-java introspection and the real byte marshaller; tests cover owner replacement, old-sender/reused-ID fencing, restored original action keys, retry and no-bus recovery with a virtual clock. Notification-server absence does not prevent tray export; watcher loss/restart re-registers. Real desktop calls remain on IO; UI callbacks use AWT.

O2b: reuse **`DesktopAppServices.trayActions`**, never instantiate another tray or DesktopBus. Register/update with `trayActions.register("backup.status", "Backup: current", enabled = true) { /* dispatch feature action; UI via AWT */ }`; unregister when the feature closes. Stable per-process IDs start at 3; Open=1 and Quit=2 remain owned by C4. Updating a key preserves its ID and bumps the layout revision. Contributions may be registered before/after startLive, carry no secret labels, and dispatch nonblocking actions. Registration is thread-safe. The runtime owns bus disposal.

## Controller-only verification still required

No emulator, live stack, real session bus or Android 30 foreground fallback ran in the implementer sandbox. Fake-bus tests do not establish compositor/niri/packaged/autostart behavior. The scripts below fail closed on missing instrumentation success, absent cache/OS delivery or missing human visible evidence. They do not substitute server HTTP tests for visible client proof.

## Controller commands

From merged root, source `scripts/env.sh`. Run `scripts/test-managed-clients.sh` (API35/36; includes C4LiveInstrumentedTest, excludes ControllerOnly) and `scripts/smoke-clients.sh porch-api35`.

On each **disposable** API35/36 device with both APKs installed, exact alarms are granted by USE_EXACT_ALARM. Do not revoke the spec permission to simulate denial. Set the POST_NOTIFICATIONS grant BEFORE instrumentation (`pm revoke` kills the app) and declare it; the test fails unless it starts in that state. Run both starts, as the M5 driver does:

```sh
adb -s "$serial" shell pm revoke app.porchlight android.permission.POST_NOTIFICATIONS
adb -s "$serial" shell am instrument -w -e class app.porchlight.android.C4PlatformControllerTest -e C4_EXACT_EXPECTED granted -e C4_NOTIFICATIONS_EXPECTED denied app.porchlight.test/androidx.test.runner.AndroidJUnitRunner
adb -s "$serial" shell pm grant app.porchlight android.permission.POST_NOTIFICATIONS
adb -s "$serial" shell am instrument -w -e class app.porchlight.android.C4PlatformControllerTest -e C4_EXACT_EXPECTED granted -e C4_NOTIFICATIONS_EXPECTED granted app.porchlight.test/androidx.test.runner.AndroidJUnitRunner
```

On API33+ notification enablement is the POST_NOTIFICATIONS runtime grant; the `POST_NOTIFICATION` app-op follows the grant and cannot disable a granted app, so the test denies by real revocation. Both runs cover denial → recovery: the `denied` run from the cold-process denial set above, the `granted` run by revoking in-process with the platform TestApi `PermissionManager#revokePostNotificationPermissionWithoutKillForTest` (shell permission identity; `am instrument` grants test-API access), the same OS state `pm revoke` produces. The API<33 path still uses the app-op and is unverified (no API30–32 AVD). Before touching private stores the test waits (60 s deadline, `am wait-for-broadcast-idle`) for startup broadcasts: API35 delivers BOOT_COMPLETED when instrumentation takes a fresh install out of the stopped state, and its locked alarm reschedule must not interleave with the fixture's legacy-ledger seed.

Real exact-alarm denial/recovery is an additional disposable **API31-32** check only (or a separately reviewed explicit test-only app-op hook). On API31-32 set SCHEDULE_EXACT_ALARM app-op `deny` BEFORE instrumentation and pass `C4_EXACT_EXPECTED denied`, then repeat with `allow`/`granted`. Revocation can kill the app, so never change it inside the test. The API35/36 automated demo/soak still tests notification denial/recovery but correctly expects exact-alarm grant.

These controller tests restore the starting POST_NOTIFICATIONS grant (re-revoking without kill after a `denied` run; the driver re-grants before the next run), the API<33 notification app-op mode and inventory tables. Use an unenrolled disposable device, not an owner's client. Channel lock-screen visibility is system/user-owned (Android stores app-created channels as `VISIBILITY_NO_OVERRIDE`), so lock-screen privacy is per notification: `VISIBILITY_PRIVATE` plus the generic public version. Safe managed checks cover channels/privacy, immutable actions, expedited inputs and both foreground-service declarations; controller checks cover actual scheduling/dumps, denied/granted posting, recreated adapters, stale snooze and restart/remote cancellation. No sleeps or UI-thread runBlocking.

Desktop: `cd client && ./gradlew --offline --console=plain --max-workers=4 :desktopApp:run` in a real niri/session-bus session. Check tray/menu/notification actions, second-launch focus, close-to-tray, watcher/notification-server restarts, no-bus visible banner, and opt-in installed launcher/autostart. Do not run against an owner's enrolled profile.

### M5 client demo and fresh-AVD soak

**Owner decision (2026-10-07):** the disposable-profile M5 client gate is replaced by real-device attestations in
[server-acceptance.md](server-acceptance.md). The demo/soak scripts below remain **laptop-dev-stack tools**, not
production gates; the existing instructions are retained to document those dev tools.

Prerequisites: running **local dev stack** (controller's normal dev-stack workflow), installed AOSP API35/36 x86_64 SDK images, KVM, real desktop display/session bus/notification server, and a **disposable** desktop XDG_DATA_HOME under `var/` already enrolled/logged in as the test recipient. No KDE wallet or other Secret Service provider is required: enroll that disposable profile with the explicit opt-in `PORCHLIGHT_KEY_STORE=file` (owner-only key file under its `porchlight/device-key/`; see [client trust](client-trust.md#desktop-key-backends-owner-decision-2026-10-07)), which it then keeps on every gate run. A profile enrolled with Secret Service instead needs that wallet unlocked (or its unlock prompt answered) when the gate starts the app. Use a test household, not household owner data. Prepare a fresh one-use Android enrollment URI for the same recipient with the emulator listener/endpoint set to `10.0.2.2` (the existing C1b enrollment controller workflow explains the listener override). Store URI/password only in a chmod-600 config under `var/`; never put them in CLI arguments or shell history. A new URI is needed for EVERY demo/soak invocation. Desktop and Android must be distinct devices.

Config shape (`var/gates/m5-client-config.json`, private placeholders, not runnable credentials):

```json
{
  "disposable": true,
  "base_url": "https://localhost:8443",
  "device_dir": "var/dev-device",
  "household_id": "<test household UUID>",
  "recipient_member_id": "<test recipient UUID>",
  "desktop_data_home": "var/gates/m5-desktop-home",
  "android_enrollment": { "uri": "<fresh enrollment URI>", "password": "<test password>" }
}
```

`device_dir` is an existing controller API credential directory (`ca.crt`, `client.crt`, `client.key`, `session.token`) for this recipient. The TLS CA must cover the config hostname. `desktop_data_home/porchlight/device.p12` is the disposable actual client's key store, not a copied plaintext key; enroll with that XDG_DATA_HOME before running, normally with `PORCHLIGHT_KEY_STORE=file` so the run needs no wallet. Scripts verify the API member and actual delivery on both devices; no credentials are printed. Gate snapshots/logs/screenshots stay under private `var/gates/m5-*` directories.

Exact commands, from merged project root in the real desktop session:

```sh
umask 022
. scripts/env.sh
scripts/test-managed-clients.sh
scripts/smoke-clients.sh porch-api35
# Demo: freshly wiped, unenrolled disposable AVD already booted; never a physical/owner device.
deploy/scripts/m5-demo.sh --client --stack var/gates/m5-client-config.json --serial emulator-5554 --keep
# Refresh android_enrollment with a NEW one-use URI first.
deploy/scripts/m5-soak.sh --stack var/gates/m5-client-config.json --api 35 --keep
# Refresh the URI again before repeating on API36.
deploy/scripts/m5-soak.sh --stack var/gates/m5-client-config.json --api 36 --keep
```

Only PASS/FAIL lines appear on stdout. `m5-demo.sh --stack` remains the inherited **server/stream-only** gate; the new `--client` mode is deliberately explicit. The soak creates a NEW uniquely named AVD under `var/gates/`, never reuses/deletes an owner's AVD, checks port 5586 is free, reaps stale netsimd before boot/after shutdown via emulator-common, and always stops its own emulator/desktop. `--keep` preserves its AVD/fixture rows/evidence, not running processes. Without it, test fixture rows and the created AVD are deleted; logs remain. Permissions and private test profile are disposable; cleanup restores server notification prefs, time/zone/network/idle/battery state and always removes pushed URI/password inputs. Interrupts run cleanup.

The driver explicitly sets exact-alarm app-op BEFORE **every** instrumentation invocation (revocation kills the app), exercises denied/granted POST_NOTIFICATIONS/exact alarms, enrolls the real device with PIN still locked, opts this disposable profile into Stay connected, creates real server reminders, verifies both cache and OS inventories, fires a real alarm offline in forced idle (optimization on), performs original-key dismissal from the Android repository and checks desktop OS cancellation, then checks held/released quiet digest on both. The soak additionally verifies OS dumps after time/zone changes and reboot, repeatedly mutates server data during network loss and checks both clients recover, keeping the desktop + Android running at least 30 monotonic minutes. Instrumentation bypasses neither trust nor UI PIN; no UI-thread runBlocking.

**Visible checkpoints are intentionally human, not inferred from cache rows.** At `visible-update`, unlock the disposable Android via its real PIN UI, open Today/Board on both devices and observe the `M5 live update` title. At `visible-dismissal`, inspect the corresponding alert has disappeared from both OS notification surfaces. At `visible-digest`, observe the quiet-hours digest on both. Each readiness PASS prints a fresh private run directory; within five minutes capture the desktop surface and attest BOTH clients in a second controller terminal, substituting that exact run path and phase:

```sh
# niri screenshot (or save the compositor's own screenshot at this filename)
grim var/gates/m5-client-<run>/desktop-visible-update.png
printf '{"android":true,"desktop":true}\n' > var/gates/m5-client-<run>/visible-update.json
# Repeat filenames for visible-dismissal and visible-digest ONLY after actually observing them.
```

The script checks fresh evidence and captures the paired Android screenshot over adb. A missing/false attestation or missing desktop PNG FAILS, never silently skips. Evidence is controller-attested, not an automated pixel assertion; review the paired images before signing off M5. A cache PASS alone is not visual proof. Desktop tray/focus/no-bus/autostart checks above are a separate required runtime checklist.

## Fix-round host evidence and controller follow-up

`PYTHONDONTWRITEBYTECODE=1 python3 -B -I deploy/tests/m5-client-gate-test.py` exercises the actual driver parsers, persisted-key dismissal precondition, R66 API-specific exact policy and the absence of Activity-start recovery crutches. Shared JVM tests call the same original-key repository dismissal function as C4GateDismissControllerTest; this does NOT substitute for the real AVD check.

The soak now checks a remote title update after reboot before any Activity startup, and after each network toggle without launching the Activity. Use the unchanged demo/soak commands above with fresh one-use enrollment URIs. Keep the device opted in (the fixture sets Stay connected). Controller should additionally turn it OFF, kill/restart the receiver/worker process, and verify no SSE/no snapshot on an alarm receiver and exactly one worker snapshot. Check sticky process-death recovery, actual boot service start, exact recovery on API31-32, packaged Start at login toggling and niri/XDG exclusivity in a real session. These runtime observations remain controller-only.

## Live runtime contract (fix rounds 2–3)

- Production heartbeat is 120 s (`docs/specs/stream.md`). Both the watchdog and the transport line-read deadline derive from `STREAM_WATCHDOG_MS`: two missed heartbeats plus 10 s (250 s). Keep server `PORCHLIGHT_STREAM_HEARTBEAT_SECS=120` (or shorter); a longer configured interval requires coordinated client constant/deadline changes and idle-stream checks, not just a server restart. An idle household must survive four heartbeat intervals without reconnect; network changes cancel the current socket and reconnect immediately. Reachability uses bounded wall-time checks (<=60 s), member-local banner time and Retry wakes the stream.
- Desktop subscribes to logind `PrepareForSleep(false)` on the system bus and immediately wakes LiveSync to reconnect the pre-suspend socket. Missing logind/system bus falls back to the watchdog; the session-bus tray remains independent. Desktop reminder waits re-evaluate wall time at least every 60 s. On resume, already-armed, undelivered reminders whose authoritative identity/time is unchanged remain deliverable within the existing seven-day local window; completed, removed or disabled reminders cancel. The spec has no separate numeric suspend grace: this uses the existing seven-day window, not a new overdue first-sync flood. Alarm-file damage is quarantined; allocation resumes above the observed code high-water mark. Atomic alarm row/counter renames fsync the file and directory. Quarantines stay private beside the ledger; review/remove them only after confirming recovery. If recovery itself fails, keep the visible storage/restart notice and disable the local alarm ledger for that process (including Snooze); never allocate codes with an unknown durable high-water. Inbox/server notification delivery stays independent when its storage is usable.
- Start at login is XDG **or** niri, never both. If niri already owns startup, an enabled XDG switch is persisted OFF once; the explanation is dismissible in Settings (not a launch notice). Close hides the window whenever the actual SNI tray is available; unrelated error notices never force show/quit. Missing tray shows the window and Close quits. Focus IPC watches only the `focus/` subdirectory and closed watchers end cleanly.
- Opted-out Android streaming follows STARTED, not focus/RESUMED. Re-entry resumes from cached cursor without an unconditional snapshot. The 15-minute worker always awaits a bounded safety-net read: notifications only while an opted-in Connected stream has a frame newer than `STREAM_WATCHDOG_MS` (API requests do not count), otherwise one full refresh; timeouts return Worker retry, while external cancellation still propagates; then alarms/delivery reconcile. BOOT/MY_PACKAGE_REPLACED start the opted-in FGS before alarm disk work. Refused FGS start queues a one-time worker; service-owned jobs cancel at destruction.
- Queued action ownership is a persisted random local generation bound to the enrolled device identity, not a session-token hash. Learning its member ID does not regenerate the owner; actual device/member identity-end still clears it. Existing device:member owner records migrate without resetting the generation. Same-identity expiry/re-login retains it; explicit logout/re-enrollment clears it. Older token-hash queues from before this change cannot safely be migrated: mismatches show a generic action failure notice. Snooze remains a local, generation-fenced action. Worker and host/controller dismissal share original-key repository dispatch.
- User-close/swipe of a server notification marks it read on both platforms. Local-alarm dismissal is local on both. Desktop failed dismissal retains a local tombstone and asks the user to mark read from Inbox (no persistent desktop retry queue). Branded Android small icon and SNI ARGB pixmap match the app; the connection/action-service public text is connection-specific, not “1 reminder”.
- Gate dismissal instrumentation force-stops its target; the fixture dismisses the keyguard and deliberately relaunches before waiting for propagation/digest. Reboot and network recovery checks still never launch an Activity. The host driver behaviour tests fake adb/time only; fake soak PASS messages are captured, never printed as CI gate evidence. API >=33 exact-denial override is logged per R66.

Controller must run fresh API35/API36 demo + 30-minute soak, direct/real broadcast reschedule, sticky process-death recovery, package replacement, BOOT/first-unlock and FGS-start denial fallback; opted-out STARTED/split-screen/permission-dialog transitions and same-identity queued action expiry/re-login; lock-screen branded reminder/connection icons/text. On real niri desktop: tray/no-tray close behaviour despite notices, one-time XDG conflict dismissal, installer minimized-line dedupe, SNI branded icon/menus/backups, server-close cross-device read propagation, lid suspend over a due reminder (within 60 s after resume), clock changes, corrupt-ledger recovery and focus/shutdown. These are not available in the sandbox.

Android cold-process reminder reconciliation preserves an already-armed just-due ticket while its authoritative reminder/time still matches, before ReminderReceiver fires it. Delivered Today and notification Snooze remain generation-fenced and work after the startup collector; completion, removal, disabled reminders and the existing seven-day bound still cancel. The shell subscribes to a zone-only distinct cache flow, not each tickle.
