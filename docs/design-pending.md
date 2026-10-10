# Client design coverage and unfinished content

The shell and Appearance/Start screen frames are **built from mockup** (`Main`, `MobileToday`, `DesktopCalendar`,
`DesktopSettings`, `MobileSettings`, `MobileSettingsToday`). Appearance and Start screen are working per-device
settings. A content placeholder is not a missing design: the tables below keep those states separate.

## C3a implemented content

Mobile Today (`MobileToday`), desktop Board (`Main`) and mobile detail (`MobileItem`) now use typed cached household data and capability-gated writes, not placeholders. C5 now adds attached lists, comments and attachments on detail. Local snooze is absent until C4 supplies it. The implemented mobile Today and desktop Board content layouts also remain DESIGN-PENDING pending a faithful mockup/layout pass; working typed data does not claim approved visual parity. C3a implements specification-based, explicitly DESIGN-PENDING desktop Today, mobile Board, desktop item detail, item create/editor and version-compare states. Common/JVM tests run deterministically; Android equivalents compile but require controller API35/36 execution. Board uses scalable columns with long-press drag, Alt+arrow keys, visible move buttons and custom accessibility actions.

## Built from mockup: frame exists, content remains a placeholder

Lists and Inbox now have working content (see C4/C5 notes below); remaining visual adaptations are DESIGN-PENDING, not approved parity.

## No design yet: DESIGN-PENDING

| Missing design / placeholder | Next owner |
| --- | --- |
| Desktop Today, Lists and Inbox adaptations | C3 / C5 / C4 |
| Collaboration entry/metadata forms, version comparison, picker/upload and recovery layouts | C5 + design pass |
| Mobile Board and Calendar, mobile Month; desktop Calendar Month and Calendar empty/error/offline states | C3 + design pass |
| Desktop item detail, item create/editor and version comparison (implemented from spec) | C3a + design pass |
| Settings compare and field-error/recovery layouts | C3 + design pass |
| Empty expanded detail-pane state | C3b + design pass |
| Notification preference detail/recovery | C4 + design pass |
| Wired Settings → Account (password, own-device revoke/logout); enrolled-device detail/admin management | C1b / C2a / C6 + design pass |
| Empty/error/offline detail states beyond the shared skeleton | Respective content owner + design pass |

## C3b working content

Calendar now consumes server-resolved agenda rows (all-day, fixed/floating labels, Busy conflicts, pending derivation). Desktop Day/Week follows DesktopCalendar's hour grid: positioned, member-tinted overlap columns, multi-member overlay, all-day/multi-day lanes and today headers. Row height scales at 200% and short blocks omit subtitles (full descriptions remain available to assistive technology). DST axes contain 23/25 elapsed hours with UTC offsets on transition days. A controller visual review is still required. Desktop Month and mobile Day/Week/Agenda are implemented DESIGN-PENDING. Mobile Month is deliberately not offered until a design pass.

Today settings sections/order/filter/week range and member color are server-backed; order has keyboard/touch move-up/down controls rather than drag. Pinned-list selection stays a C5 placeholder and cannot be enabled without a saved list ID. Appearance and start screen remain device-local; Account retains C1b/C2a wiring; Notifications stays the C4 placeholder. Error/offline/compare layouts remain DESIGN-PENDING.

Resume includes board filter, calendar view/date/primary member/overlay, selected list, item ID and settings section. C3b supplies local-only `AppServices.routeAvailable` for deleted/invisible entities, including cold disk initialization. Unknown selected lists fail closed until C5 supplies cached ACL rows. Only Evening has designed mockups; Daylight remains
a token-derived accessible shell pending its visual design pass.

## Desktop backup status (O2b): DESIGN-PENDING

Settings → Account adds a super-admin-only backup panel: last successful run, next server due, controlled
failure indication, current-session verified local bundle, pull progress/retry and backup-connectivity hint.
It is not an approved mockup. The C4 hook is `AppServices.backups.state.unreachableSince`; C4 owns stream outage
thresholds/desktop notifications. See operations/passive-pull.md for lifecycle and authority boundaries.

## Trust and onboarding screens (C1b): DESIGN-PENDING

C1b implements the following accessible Material 3 trust screens from the specification, not approved mockups. They are marked `DESIGN-PENDING` in source and need a later Claude Design pass. All shared forms scroll, use labeled fields and minimum 48 dp action targets; generated JVM UI tests include 200% font scale.

- Enrollment: paste URI, Android QR scanner/camera permission recovery, emulator ENROLL_HOST hint.
- Login and session-expired re-login.
- Reset-code entry and password reset.
- Password step-up confirmation dialog.
- Change password.
- Credential-only Android PIN gate and missing/cancelled device-credential recovery.
- Wired permission onboarding and notification/exact-alarm/battery denial recovery, persisted “Continue with limited reminders”, loading and save-error/retry states.
- Upgrade-required screen.
- Re-enrollment-required screen.
- Locked/unavailable Secret Service wallet recovery (automatic activation does not supply an approved recovery design), including the R85 enrollment-time choice "Store the key in a protected file instead" with its disk-encryption note, and the enrollment screen's "This device's key will be kept in: …" line (deskkey).
- Desktop "Key folder unusable" recovery (unsafe owner-only key folder: names the folder and reason, Retry only; deskkey).
- Local-trust-loss recovery: non-destructive “Try again” before clear/re-enrollment.
- Own devices, revoke and logout, with the desktop line naming which backend protects this device's key (deskkey).

Source ownership: `client/shared/src/*/kotlin/app/porchlight/shared/{trust,onboarding}/`, and `client/androidApp/src/main/kotlin/app/porchlight/android/onboarding/AndroidQrScanner.kt`. C2a has integrated the gate, credential-first Android launch wrapper and Settings Account panel on both platforms without claiming an approved design. Existing DESIGN-PENDING markers in the trust/permission source still apply to the wired states; no additional household-content screen was implemented by C2a. The deterministic debug-only shell fixture is test support, not a product screen.

## C4 working content (milestone gates still incomplete)

Inbox now renders grouped Today/Yesterday/This week/Earlier rows from typed, recipient-private cache data, safe server summary topics, read/dismiss/open actions and canonical notification preference toggles/quiet hours. MobileInbox informs the grouped structure; desktop adaptation, preference recovery, exact-alarm degraded hints and reachability banner remain DESIGN-PENDING. Both-theme/double-font/48dp Inbox/banner JVM checks are present. Android runtime adapters and desktop D-Bus adapters compile; controller device/session-bus tests and M5 client gates/soak are still required. This is not approved visual parity or an M5 completion claim.

## C6 working calendar/planning content — DESIGN-PENDING

ICS preview/commit, feed administration, weekly availability editor, mobile/full EDF cards and dependent candidates, export/save, and super-admin device list/revoke are implemented from spec with accessible Material forms. Desktop Calendar suggested-time cards follow its panel content; availability shading/member-zone DST bands need visual review. These forms, recovery/compare/password/file-dialog states and both mobile layouts have no approved frames and remain DESIGN-PENDING.

Availability date exceptions/independent load limits are not part of the weekly-window server contract and are explicitly unavailable, not mocked. R65 device certificate expiry and invalid/cancelled import breakdown are now rendered. Feed target/interval editing, picker-return unlock, populated planning/device rows and both-theme 200% typography remain pending visual review. Settings Account retains own-device logout. See `docs/operations/client-planning.md` for interfaces and controller checks.

## C5 working collaboration content

Mobile Lists and the MobileItem comments/attachment sections now use typed server models and capability-gated online writes. The structure follows MobileLists (open entries / Got it) and MobileItem (thread / attachments); visual parity is not asserted. Standalone and item-attached persistent/per-occurrence lists, neighbour moves (long-press and accessible actions), Today pinning, mentions, author edit deadlines, file uploads/cancel, private downloads and thumbnail/system-viewer actions are implemented. Desktop Lists adaptations and collaboration forms/compare/recovery are DESIGN-PENDING. Android intent/content tests compile; controller API35/API36 execution and real picker/camera/system-viewer checks remain required. Desktop captures are not offered; it uses the file chooser.

## Trust recovery and Recents (ctrust): DESIGN-PENDING

Enrollment recovery preserves the exact attempt in encrypted secure storage across process death. Re-entry uses the stored key/CSR; the server decides changed-password outcomes. R78 clears definitive/never-sent attempts and adds a local-only Start over action (no token/password/password-verifier persistence). Recovery/cancel presentation remains DESIGN-PENDING; do not add token/password persistence for convenience. Certificate renewal is scheduled with bounded retry backoff, including while the UI-only PIN gate is locked. R78 intentionally gives Porchlight a blank/redacted Recents card: every activity disables snapshots on API ≥33 and secures paused/stopped windows on all APIs, including content beneath attachment/system sheets. Foreground user screenshots remain permitted, and UI relock stays at ON_STOP.

## C4 fix round 2 pending notification interactions

“Take it” (claim an unassigned appointment from a notification) is not implemented. “Failures update the notification with a retry” is not implemented: Android writes a durable generic in-app failure notice and retries transient Worker failures; desktop asks the user to retry from Inbox. Both require a designed action/recovery state and runtime checks, not an M5 visual-completion claim. The first-sync unread OS flood cap (m-5) also remains pending; no arbitrary cap is introduced here. Branded icons, member-local reachability time, readable Inbox topics, actual-tray close behaviour and the one-time dismissible niri/XDG settings conflict are working states, still subject to controller visual review.

## C4 fix round 3 recovery states — DESIGN-PENDING

Cold-process Android reminders now preserve local Snooze through startup reconciliation. Desktop logind resume wakes live sync; unavailable logind retains watchdog fallback. Alarm-store recovery failure keeps an explicit storage/restart notice and a disabled local alarm ledger rather than ending the notification runtime. Persisted niri switch-OFF/conflict copy, zone-only reachability updates and Worker timeout retries are host-tested behaviour, not approved visual parity. Real API35/API36 cold-receiver/keyguard and logind/lid-resume checks remain controller-only; no M5 device/session gate or soak completion is claimed.
