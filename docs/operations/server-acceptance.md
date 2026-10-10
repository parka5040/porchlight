# Production server acceptance (M9)

Human-attested gate for **the production server and the owner's real devices**. Nothing below is pre-attested by offline CI.
Use [server-install.md](server-install.md) for LAN deployment, [devices.md](devices.md) for each physical phone,
and [m8-acceptance.md](m8-acceptance.md) for the two-release laptop acceptance gate. Use
[server-install.md — Funnel](server-install.md#funnel-after-tailscale-pre-registration) and
[docs/operations/funnel.md](funnel.md) for Funnel; LAN success alone is not full M9 sign-off.

The owner's 2026-10-07 decision moves M5 visible desktop update/dismissal/digest attestations here. Do **not** run
M5 demo/soak, disposable-profile drivers, instrumentation fixtures or destructive tests against production.
Use owner-approved harmless test tasks/reminders through real client UIs; restore changed preferences afterward.
Keep evidence privately under `var/m9/acceptance/<run>/` (directory 0700, files 0600). Reference evidence paths
and UTC observation times below; never include passwords, PATs, enrollment URIs, keys, `.env` or private household
content in this document. Screenshots must avoid clipboard, enrollment and credential screens. An unchecked or
blocked item is not a PASS; cache rows/HTTP responses alone are not visible client proof.

Run/date (UTC): __________  Controller/observer: __________  Signed release version(s): __________
Private evidence directory: __________  Enrolled device record IDs: __________

## 9.1 — Host and LAN prerequisites

- [ ] Owner applied `deploy/nixos-porchlight.nix` and rebuilt the NixOS host; SSD is mounted, Docker has
  `RequiresMountsFor=<data-mount>`, required tools/group membership are present, and the firewall permits
  LAN TCP ports `8443`/`8444` (not private `9443`/`5432`).
- [ ] `<data-mount>/porchlight` and `/run/porchlight` are `0750 <server-user>:porchlight`, matching the host module
  contract. No competing Compose project named `porchlight` uses another root.
- [ ] The laptop resolves `porchlight.lan` to `<server-lan-ip>` via `/etc/hosts`; phones use
  [enroll-by-IP](devices.md#phones-without-lan-dns). SSH host-key trust and owner recovery/signing-key backups
  are established without copying private keys to the server.
- [ ] `scripts/deploy-server.sh preflight` passes every check. Evidence/time: __________

## 9.3 — Signed deployment, fresh init and enrollment boundary

- [ ] Controller deployed a trusted signed `dist/<version>/`; transport hashes, pinned manifest verification,
  image digest verification and API health passed. `scripts/deploy-server.sh status` shows the intended active
  release and healthy services. No laptop database or PKI was imported.
- [ ] Fresh household CA fingerprint recorded privately and compared at enrollment. Fingerprint evidence path:
  __________ (not the bootstrap token or `init.json`). Required LAN/localhost/IP SANs are present.
- [ ] LAN mTLS is on 8443; enrollment is on LAN-only 8444 with `ENROLL_CIDRS=<lan-cidr>`;
  Postgres binds only `127.0.0.1:5432`; Funnel target 9443 remains unpublished.
- [ ] Main laptop enrolled successfully using its fresh URI; the **API enrollment log** identifies the laptop's
  actual `<lan-cidr>` LAN address as peer, not a Docker gateway. Correlate timestamp/device with the successful
  enrollment and retain only sanitized peer evidence. Actual peer / log evidence / UTC: __________
  Do not widen the CIDR to mask lost source addresses.
- [ ] Main laptop uses the same signed desktop release, dev data was renamed rather than deleted, CA trust was
  checked before credentials, and Secret Service or the protected-file key backend was chosen explicitly.
  Consumed URI/clipboard history cleared; bootstrap/admin PAT retained in protected custody as needed.

## Main laptop — moved M5 desktop attestations

Use a second enrolled real client for remote changes/dismissal (record its device ID). If not yet available, leave
these unchecked until phone enrollment. Observe the **real desktop UI and OS notification surface** in its normal
niri/session-bus session; keep timestamped screenshots/observer notes in the private evidence directory.

| Attestation | Human observation required | UTC / evidence / observer |
| --- | --- | --- |
| [ ] Visible remote update | Change an owner-approved task on the other device; the main laptop Today/Board shows the new title/state without manual reload. | __________ |
| [ ] Visible dismissal sync | Deliver a server notification to both devices, then dismiss/mark it read on the other client. Desktop OS alert disappears and Inbox read state agrees; also verify desktop dismissal propagates back. Local-only alarm dismissal is not this test. | __________ |
| [ ] Visible quiet-hours digest | Temporarily configure approved quiet hours; notifications are held, then the quiet-hours batch digest appears on the desktop OS surface when quiet hours end. Restore preferences. | __________ |

- [ ] Tray/menu and notification actions work; second launch focuses the existing instance; close-to-tray works
  even with notices, while no-tray close quits. Branded SNI icon and backup menu work.
- [ ] Watcher/notification-server restart recovery, focus/shutdown, visible no-bus banner and installed launcher
  work. Opt-in Start at login uses **XDG or niri, never both**; conflict explanation dismisses once and the
  installer does not duplicate minimized startup lines.
- [ ] Lid suspend over a due reminder delivers/reconciles within 60 seconds after resume; approved clock/time-zone
  changes reconcile reminders. Corrupt-ledger recovery is checked only with an owner-approved private copy/recovery
  procedure, never by blindly damaging the owner's live ledger. Evidence/time: __________

## 9.2 / 9.3 — Funnel (Task 4; controller/owner gate)

- [ ] Owner completed Tailscale pre-registration: HTTPS and MagicDNS enabled, `tag:porchlight` plus tagOwners,
  Funnel nodeAttr, neutral machine name/randomized tailnet name, single-use pre-approved tagged auth key
  (tagged node key expiry off). Key kept private; auth-once leaves no reusable auth key in Compose/runtime config.
- [ ] Vetted Tailscale image pulled/verified by immutable digest. Controller ran
  `scripts/deploy-server.sh funnel --ts-hostname <name> --tailscale-image <digest-ref>`; valid
  exit success plus `PASS funnel <fqdn>` and `var/m9/funnel-fqdn` recorded. Server transport key removed
  on every remote exit (success or failure); owner deleted the laptop source after success.
  Idempotent wrapper `--no-authkey` (no new laptop key; reuse staged key or saved node state) retains the FQDN.
- [ ] `scripts/deploy-server.sh funnel-check` (or `--fqdn <name>`) passes every status/private-state/SAN check:
  `compose-valid`, `api-health`, `tailscale-running`, `authkey-cleared`, `raw-tcp-proxy-v2`, `leftover-authkey`,
  `private-state`, `production-sans`, `remote-status` and `household-ca`. Running; no runtime/staged/transport
  key or persisted auth reference; correct private modes/enabled endpoints; live raw TCP 443 → `api:9443`,
  PROXY v2 and AllowFunnel. `FAIL status-transport` means SSH exit 255, not a failed remote assertion.
  `production-sans` creates a one-off verification container, so this command is not strictly read-only.
  Evidence/time: __________
- [ ] **Separate live evidence** (not asserted by `funnel-check`): sidecar userspace mode and static
  `172.29.251.10` network IP verified using narrow inspection (never `Config.Env`); no host 9443 listener via
  `ss -tlnH`, no published Compose 9443 port, and direct LAN access is refused. API accepts PROXY v2 only
  from the trusted sidecar. Evidence/time: __________
- [ ] M9-R5 `funnel-check` run **off-tailnet** prints `PASS probe-path` and `PASS public-tls`: isolated Python
  stdlib SSL completes a household-CA + FQDN SAN verified TLS 1.3 client handshake, then its first recv raises
  `TLSV13_ALERT_CERTIFICATE_REQUIRED` (or `CERTIFICATE_REQUIRED`). No diagnostic-text matching; the Python
  probe exits 0 only on this verified proof. Any tailnet DNS answer fails `probe-path`; chain/name failure,
  data/EOF without that alert, unexpected errors or timeout fail `public-tls`. No client certificate or
  unauthenticated HTTP health call. Private probe evidence/time: __________
- [ ] Household CA is unchanged; server leaf adds the actual ts.net FQDN; client endpoint list contains both
  `https://porchlight.lan:8443` and `https://<tailscale fqdn>`. LAN still works after activation.
- [ ] Phones refreshed endpoint lists on LAN before Wi-Fi was disabled. Cellular mTLS/session succeeds over
  raw-TCP Funnel through the host VPN (if any); connection metadata reports `transport=funnel` and forwarded cellular source.
  Cellular CRUD, SSE reconnect/update, attachments, notifications and designated-test-device revocation pass
  (see 9.4). Enrollment remains LAN-only; no device-key export, TLS-termination shortcut, widened peer CIDR
  or unapproved egress bypass was used. Evidence/time: __________
- [ ] Human crt.sh check for the chosen public name recorded privately (query/date/result): __________
  Explain expected public-name exposure to the owner; never submit private household material.

Restore deliberately leaves the stack LAN-only; `funnel-check` must fail until the controller explicitly
re-enables Funnel (`funnel ... --no-authkey` with surviving staged key/node state, or with a new owner key
when none survived). Recheck the restored LAN API first. Merge/release installer part A and its
`funnel.md` runbook before this live gate; a part-B-only tree does not provide the server modes/override.

If raw-TCP Funnel through the host VPN fails, **stay LAN-only**, record the failure, and bring options to the owner.
Do not claim Funnel or cellular acceptance, or change egress without the owner's decision.

## 9.4 — Real household phones and data out

### Phones without LAN DNS

Requires a release whose installer has `enroll-host`; run `scripts/deploy-server.sh upgrade <version>` first.
Enable/verify Funnel **before** phone enrollment, then run from the laptop:

```bash
scripts/deploy-server.sh enroll-host <server-lan-ip>
scripts/deploy-server.sh enroll-uri --device phone-1 --user admin --out var/m9/phone-1-enroll.json
```

Use the intended member/device and keep the URI private. On the allowed LAN the phone enrolls through
`<server-lan-ip>:8444` (IP SAN required). It then syncs through Funnel when `porchlight.lan` fails to resolve;
keep the published LAN-name/Funnel endpoint list, **not** a private-IP sync endpoint. See
[phone instructions](devices.md#phones-without-lan-dns). Existing URIs must be regenerated after changing the host.

- [ ] Phone with no LAN DNS: `porchlight.lan` does not resolve (NXDOMAIN) on the phone's home Wi-Fi;
  phone enrolled by IP and synced over Funnel on that Wi-Fi. Record resolver result, UTC and private evidence.

- [ ] Each household phone installed the signed release APK (`adb install`), scanned a real fresh QR on the LAN,
  compared the CA fingerprint and enrolled. Complete a separate [device record](devices.md) for every phone.
- [ ] Per phone: actual `specialUse` FGS start at boot and first unlock, recorded OEM battery settings/reference,
  exact alarm in overnight idle, notification permission behaviour and lock-screen actions/privacy attested.
  Observe real background behaviour without launching the Activity to fake recovery.
- [ ] Visible update, cross-device server-notification dismissal and quiet-hours digest observed on real phones;
  retain private evidence and correlate with the laptop observations above.
- [ ] With Wi-Fi off, each phone proves cellular CRUD, SSE updates/reconnect, attachment upload/download,
  notifications/actions and revocation (use a designated owner-approved revocable test identity, not the sole admin).
  Revocation closes its stream and prevents further access. Device record/evidence: __________
- [ ] Main laptop passive pull verified with a matching receipt and an independent restore as described in
  [passive-pull.md](passive-pull.md); exports/backups are usable, not merely downloaded.

## 9.5 — Upgrade and resilience drills

Schedule disruptions with the owner; preserve recovery artifacts and keep private identities on the laptop.

- [ ] Controller ran the M8 two-signed-release acceptance on the **laptop**, then the production upgrade drill:
  preflight, signed upgrade, pinned pre-upgrade backup retained, intended active release healthy, clients reconnect
  with household CA unchanged. Record versions and backup/evidence paths: __________
- [ ] VPN relay change (if the server uses one): server egress still uses the VPN; LAN and (if enabled) Funnel clients recover, SSE and
  reminders continue. No unapproved routing exception. Evidence/time: __________
- [ ] Server/stack restart: API/backup scheduling recover and real clients reconnect. Evidence/time: __________
- [ ] Full server reboot: mounted SSD precedes Docker; `/run/porchlight` is recreated with required owner/mode,
  agent socket works and API/backup services are healthy. Evidence/time: __________
- [ ] `scripts/deploy-server.sh backup` creates a named encrypted bundle; controller copies **ciphertext only** to
  private laptop `var/`, compares remote/local SHA-256 and runs the matching signed release's isolated
  [backup→laptop restore drill](server-install.md#statusbackup). DB rows/migrations, CA and referenced blobs match,
  API serves successfully. No production overwrite or private-key upload. Bundle/version/evidence: __________

## Sign-off

All required items above and all household phone records must be completed with observed evidence for full M9.
LAN-only operation pending Funnel is a documented partial acceptance, **not** full M9 completion.

Result: [ ] Full M9 accepted  [ ] LAN-only partial acceptance  [ ] Blocked
Outstanding items / owner decisions / evidence references: __________
Owner signature/name: __________  Controller signature/name: __________  Date (UTC): __________
