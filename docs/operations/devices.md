# Household device acceptance records (M9.4)

Complete one copy of the table per **real household phone**, then reference its record ID in
[server-acceptance.md](server-acceptance.md). These are human observations, not emulator/CI results. Leave
untested fields pending; record failures and the actual setting/workaround rather than assuming a vendor default.
Do not run M5 disposable-device instrumentation/demo/soak against an owner's production profile.

Keep filled records and screenshots/logs privately under `var/m9/devices/<record-id>/` (0700 directory, 0600 files).
This checked-in template contains no credentials or personal device identifiers. Use a neutral record ID, not a
serial number/IMEI, account address, PAT, enrollment URI or key. Record UTC times and evidence paths. Screenshots
must not expose QR/credential screens or private household content.

## Phones without LAN DNS

The laptop's `/etc/hosts` entry does not make `porchlight.lan` resolvable on phones. **Enable and verify
[Funnel](funnel.md) first**: phones enroll via the LAN IP but then sync via Funnel, at home and away.
Requires a release whose installer has `enroll-host`; run `scripts/deploy-server.sh upgrade <version>` first.
From the laptop, with the phone on the allowed LAN (`ENROLL_CIDRS=<lan-cidr>`):

```bash
scripts/deploy-server.sh enroll-host <server-lan-ip>
scripts/deploy-server.sh enroll-uri --device phone-1 --user admin --out var/m9/phone-1-enroll.json
```

Use the intended household member's username and a fresh URI for each phone. Keep the saved URI private;
scan/paste it in the app and compare the household CA fingerprint before sending credentials. The new URI
uses `<server-lan-ip>:8444`; the current server certificate must have IP SAN `<server-lan-ip>` (otherwise use
`install.sh server-cert` with the complete SAN set first). This does not expose enrollment through Funnel.

Keep published endpoints `https://porchlight.lan:8443,https://<node>.ts.net`: do **not** add the private LAN IP
to `PORCHLIGHT_ENDPOINTS` (it would impose a connect timeout away from home). A name-resolution/connect failure
on `porchlight.lan` advances to the Funnel endpoint, which sorts last. TLS/hostname failures do not fail over.
Record actual phone enrollment and sync observations below; JVM regression tests are not physical-phone acceptance.

## Per-phone template

| Field / check | Setting, result, UTC and private evidence reference |
| --- | --- |
| Record ID / observer / test date | __________ |
| Model / manufacturer | __________ |
| Android version / API / build / security patch | __________ |
| Signed Porchlight release / APK versionCode | __________ |
| LAN enrollment | Real camera QR scan, fingerprint comparison and successful enrollment: __________ |
| OEM battery setting | Exact vendor menu path and selected optimization/background/autostart setting: __________ |
| OEM reference | Relevant dontkillmyapp.com vendor page and date consulted (guidance, not proof): __________ |
| Stay connected / notification permissions | Opt-in state; POST_NOTIFICATIONS grant/denial and recovery observed: __________ |
| `specialUse` FGS at boot | Actual boot/BOOT_COMPLETED behaviour with Activity unopened; service/connection notification evidence: __________ |
| FGS at first unlock | Actual first-unlock behaviour, PIN/UI gating and background reconnection without Activity launch: __________ |
| FGS/process recovery | Process death, denied-start fallback and opted-out behaviour if checked: __________ |
| Exact alarm authorization | Device/API's actual permission policy and grant; applicable denial/recovery or reason not applicable: __________ |
| Exact alarms overnight idle | Armed time / overnight interval / idle and optimization state / due time / actual delivery time; no Activity-start recovery: __________ |
| Lock-screen actions/privacy | Branded reminder and connection icons/public text; server alert dismiss/read, completion and Snooze; no private title leak: __________ |
| Visible update | Remote change visible in Today/Board without manual reload: __________ |
| Visible dismissal sync | Server-notification dismissal propagates phone↔desktop and OS alerts disappear (not local-only alarm dismissal): __________ |
| Quiet-hours digest | Held notifications and visible batch release; preferences restored: __________ |
| Cellular CRUD / SSE | Wi-Fi off; create/read/update/delete approved test item, remote live update and reconnect: __________ |
| Cellular attachments | Upload/download verified bytes on cellular: __________ |
| Cellular notifications/actions | Delivery and actions while backgrounded/locked: __________ |
| Revocation | Approved test identity/device revoked, stream closes and access fails; replacement enrollment if needed: __________ |
| Notes / limitations / remediation | __________ |
| Result / sign-off | PASS / FAIL / PENDING; owner/observer and UTC: __________ |

Repeat for every household phone. Do not revoke the sole working admin device to satisfy a test; use an
owner-approved designated revocable identity/device and mint a new one-use LAN URI if re-enrollment is needed.
Battery-setting documentation never substitutes for an observed overnight idle alarm or real boot/first-unlock FGS.

## Main laptop record (companion)

Desktop visual attestations live in [server-acceptance.md](server-acceptance.md); Android-only fields do not apply.

| Field | Private record |
| --- | --- |
| Record ID / signed release / UTC / observer | __________ |
| Desktop/session/notification provider | __________ |
| Chosen key backend | Secret Service provider or protected file; no key material: __________ |
| Dev profile renamed / production LAN enrollment / fingerprint verified | __________ |
| Startup owner | XDG or niri (never both): __________ |
| Acceptance evidence references / notes / result | __________ |
