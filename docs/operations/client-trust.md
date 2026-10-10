# Client trust and app wiring (C1b / C2a)

Both production entry points use `TrustedPorchlightApp`: trust and onboarding gate the restored shell, and Settings → Account exposes `TrustAccountScreen` only after trust passes. Android checks device credentials before enrollment/login too. Everything is laptop-only until M9.

## Integration contract

Common:

- `TrustController(keys, factory, platform, now, pinRequired)` owns enrollment, restored sessions, auth, own-device management, certificate renewal and the gate state. `platform` is exactly `android` or `desktop`; inject `kotlin.time.Clock.System.now` and `ClientInfo.version` from the release version.
- `TrustGate(controller, android, pinGate, onboarding, scanner, content)` renders enrollment → login → required PIN → permission onboarding → restored content. The default required-PIN slot **fails closed**; never replace it with a direct content call.
- `TrustAccountScreen(controller)` supplies password change, own devices, step-up/revoke and logout, outside the navigation shell.
- `controller.transport()` is an `ApiTransport`; requests accept session, caller-supplied idempotency key, `If-Match` version and bound step-up token. Use `controller.sessionToken()` for authenticated requests. C2/C4 report generated-client problems via `controller.onProblem(problem)` and TLS revocation via `controller.onTransportError(error: DeviceCertificateRevoked)` so the trust gate shows expiry/re-enrollment/upgrade states. Do not construct an ordinary system-CA HTTP client for generated requests.
- `KtorApiTransport` selects LAN origins before `.ts.net` Funnel origins, preserves server order within each tier, and retries only definite connect/DNS failures. A failover keeps the exact mutation body and idempotency key. TLS, hostname, HTTP, cancellation and ambiguous read/write errors are not retried. There are no redirects or Ktor/OkHttp request loggers.
- Endpoint origins must be HTTPS, without credentials, query, fragment or non-root path. Every request carries version and a supported IANA zone (legacy fixed/SystemV names are mapped, or the zone header is omitted with a warning); non-GET defaults to a fresh secure 128-bit base64url key. SSE line-read/watchdog deadlines derive from `STREAM_WATCHDOG_MS` (250 seconds for the assumed 120-second heartbeat); keep `PORCHLIGHT_STREAM_HEARTBEAT_SECS` at 120 or shorter unless coordinating a client change. Binary export idle reads retain their separate 300-second deadline.
- `ApiProblem.type` is the server slug string, `status/title/detail/extras` retain problem+json data, but exception diagnostics redact detail/extras. Do not log JSON bodies or tokens.
- The M2 enrollment request intentionally has **no label**: the server rejects unknown fields and obtains its device label from the enrollment token. Malformed percent escapes are rejected with secret-free diagnostics; otherwise the parser mirrors Rust's permissive URI-value parsing; strict target/IPv6/port validation happens before connection.
- Enrollment commits a pending attempt **before the first send**: secure-random 128-bit idempotency key, exact randomized CSR, key alias and SHA-256 **target** binding, encrypted in the platform store. No token, password, password verifier or private key bytes are serialized. After a lost reply/process death, re-enter the original URI/password to replay the sealed success. The effective recovery window ends at the earlier of idempotency expiry (24 hours) and the **next nightly purge of expired enrollment tokens** (15-minute TTL): token lookup precedes replay, so this is not a guaranteed 24-hour window (roughly 15 minutes to 24 hours depending on purge timing). Changed credentials are sent under the stored key: a processed different body returns `idempotency-key-reused` (422), which only proves the first request was processed, not that it failed, so the attempt is **kept** and the user is told to enter the original password or choose Start over. Governing rule (R78/R78a): a pending attempt is deleted only when it can no longer be useful, i.e. its result was saved locally, the server gave a definitive answer proving this attempt can never succeed (the stored-attempt rejection allowlist below), or the user chose Start over. For an attempt created by the current call, any 4xx except `idempotency-key-reused` or `idempotency-in-progress` clears it (including other 422); stored attempts use the narrower allowlist. Anything that may leave the server outcome committed keeps it: post-write timeout/reset, 5xx, cancellation, 409 `idempotency-in-progress` (bounded retry delay), a locked wallet, a malformed success body, and a received 2xx whose local save failed (an ordinary save failure preserves the prior state, normally Enrollment, for exact replay; `WalletUnavailable` enters WalletLocked, then restore reloads local state after unlock before retry; neither path discards the pending attempt as an unknown local-trust failure). A server rejection of a **stored** attempt before processing (429 `rate-limited`, 408, 426 `upgrade-required`, header/body problems) says nothing about an earlier send, so the attempt is kept with retry backoff (426: update the app, then retry); only `enrollment-token-invalid/-used/-expired`, `invalid-credentials`, `account-locked`, `unauthenticated` and `csr-invalid` clear a stored attempt. On Android, pending metadata is dropped on load only for definitive unreadability (missing key alias, AEAD/authentication failure, malformed or foreign metadata); a transient Keystore/provider error keeps it and surfaces a retryable error. A provably-unsent connect/DNS/TLS-handshake failure clears only an attempt that the same call created; an attempt loaded from storage may have committed on an earlier send, so it survives an offline retry. Different tokens do not erase unrelated unknown attempts. Successful trust commit atomically removes pending attempts. Enrollment's **Start over** clears only local pending attempts and an unused key, never enrolled trust or server state; a token already consumed on the server still needs replacement.
- Session refresh stores `/v1/me`'s sliding `session_expires_at` (Android 180 days, desktop 90 days; these are **session**, not certificate windows). `refreshWhenReady()` starts one application-lifetime obligation: initial refresh, a timer strictly under the 30-day certificate heuristic, and at most daily session/endpoints checks. Failures retry after 1 minute, doubling to a 1-hour cap. Only failures within 7 days of certificate expiry show a persistent near-expiry warning, cleared on successful renewal/refresh; earlier transient failures do not nag. Server 409 `renewal-too-early` is **not due**, using daily cadence rather than escalating backoff; newly issued short-lifetime certificates also use daily cadence. Foreground (Android activity resume/desktop window focus) and PIN unlock recheck overdue wall-clock deadlines after suspend without resetting the existing retry deadline/backoff. R64 permits renewal while PIN-locked; missing/expired sessions, revocation and upgrade gates park it. Refresh network reads hold neither UI busy nor the controller mutex; late replies are fenced against identity/session changes. Only certificate/CA/endpoints changes, or a missing transport, rebuild the live connection. C2/C4 must not retain old certificate-bearing connections.

Android construction (see `androidApp/.../MainActivity.kt`):

```kotlin
val services = AndroidAppServices.get(applicationContext)
val controller = services.trust.controller // application-scoped keys/controller/action scope
setContent {
    PorchlightAndroidRoot(services, controller, scanner = { AndroidQrScanner(it) })
}
```

`AndroidLaunchTrustGate` includes `AndroidTrustLifecycle` and discards its trusted UI subtree on ON_STOP; it is not a restorable unlocked flag. A saveable state holder above the gate retains the shell's navigation/back stack without composing private content. Application-scoped keys/controller and controller-owned mutations survive activity recreation. The encrypted pending enrollment also survives **process death**, retaining byte-identical CSR and idempotency key; the URI/token/password must be supplied again and are not persisted. Gate recomposition does not restart restore or the scheduled refresh obligation. Cancellation/missing credentials remains fail-closed. Rotation still requires the credential again (no `isChangingConfigurations` exemption); opaque Android Settings pages still relock. This intentionally conservative credential/form UX remains DESIGN-PENDING. “Continue with limited reminders” is persisted in per-device DataStore before content is admitted, including cold starts. A write error keeps onboarding visible with retry. Local trust loss offers **Try again** (reload without clearing) before destructive clear/re-enrollment. Device zones are cached until the system zone changes; supported slashless IANA IDs such as CET and UTC are sent.

The manifest's `PorchlightApplication` registers task-snapshot callbacks for **every** activity, including MainActivity, AttachmentPickerActivity, credential/notification helpers and debug/future activities. R78 disables Recents screenshots on API ≥33 before creation content and applies `FLAG_SECURE` on **all APIs whenever not RESUMED**: pre-create/pre-pause/pre-stop secure the window, post-resume clears it. This also secures private content underneath photo-picker sheets and permission dialogs while foreground user screenshots remain possible. PIN relock stays at ON_STOP. Host tests drive the actual lifecycle policy and manifest activity inventory (not source grep); `TaskSnapshotInstrumentedTest` checks real callback/window dispatch. Actual Overview cards remain controller-only.

The QR scanner lives in `androidApp`, where CameraX/ZXing are already declared. A private `FragmentActivity` hosts `BiometricPrompt(DEVICE_CREDENTIAL)` so the existing MainActivity need not change its superclass. Private EC P-256 keys are not authentication-bound and never exported; a custom key manager signs through AndroidKeyStore. Session/certificate metadata is AES-GCM encrypted with a separate non-auth-bound Keystore key. Backup and cleartext network traffic are disabled in the manifest.

Desktop construction uses `DesktopKeyStore()` and `OkHttpTrustFactory(keys, ClientInfo(version))`, `platform = "desktop"`, `pinRequired = false`, and the common `TrustGate` (no PIN/onboarding/scanner slots). Default storage is `$XDG_DATA_HOME/porchlight/device.p12` or `~/.local/share/porchlight/device.p12`, mode 0600 in a 0700 directory. The PKCS#12 snapshot encrypts key, trust metadata and pending enrollment attempts together; a fsync + atomic rename commits replacement. Its random password lives in exactly one of two backends, chosen at enrollment and recorded (next section): Secret Service (service `app.porchlight.device`, account `pkcs12-password`), or an owner-only key file when no Secret Service provider exists. A locked/unreachable Secret Service offers wallet unlock/retry. A missing key secret with an existing PKCS#12, corrupt metadata or missing Android Keystore keys instead offers clearing local trust and re-enrolling; it never loops on wallet retry. For Secret Service, `java-keyring` is preferred; the native dbus-java 5 adapter is the fallback, using the same `service`/`account` attributes. Runtime verification of both adapters is controller-only.

## Desktop key backends (owner decision 2026-10-07)

Desktop key storage is desktop-agnostic: nothing requires KDE.

- **Secret Service (primary).** Any provider of `org.freedesktop.secrets` (GNOME Keyring, KeePassXC, KWallet/ksecretd) with a default collection. When the default collection is **locked**, Porchlight calls the standard `Unlock`; the provider shows its own prompt. Porchlight runs the returned prompt (`Prompt.Prompt("")`) and waits for its `Completed` signal for at most 120 seconds (a timeout calls `Dismiss` and counts as dismissed). The secret is read only on startup restore, at enrollment and on **Retry after unlock**; the opened PKCS#12 stays in memory for the process, so background renewal and sync never prompt. Each attempt (startup restore, enrollment, **Retry after unlock**) shows at most one unlock prompt. A prompt that is dismissed, times out, fails once shown, or completes without unlocking the collection is final for that attempt and leaves the recoverable **WalletLocked** state. **Retry after unlock** prompts again. A `CreateItem` prompt is accepted only if the new secret then reads back. There is never an automatic fallback to the file.
- **Owner-only key file (no provider).** At enrollment a provider **exists** when `org.freedesktop.secrets` or `org.kde.secretservicecompat` has an owner, or when one activation attempt (next section) starts either name. A provider that starts but does not claim the standard name within about 4 seconds still counts as existing. The file is chosen automatically **only** in two cases. Either nothing is activatable: both names are `ServiceUnknown` and neither has an owner. Or the session is genuinely without a session bus: no `DBUS_SESSION_BUS_ADDRESS` and no `$XDG_RUNTIME_DIR/bus` socket, or a configured `unix:path=` socket that does not exist. Any other connection failure (refused, timed out, or the D-Bus transport missing from a package) means a provider may exist but cannot be used. In those automatic cases the password goes to `<data>/porchlight/device-key/pkcs12-password`. That folder is 0700 and the file is 0600, created atomically (0600 staging file, fsync, rename, folder fsync) by `DesktopPrivateFiles`. A symlinked, non-folder, other-user, unreadable or non-POSIX folder, or a symlinked or other-user key file, fails closed with `PrivateDirectoryUnavailable`. The desktop gate then shows **Key folder unusable**, naming the folder and the reason, with **Retry**; it never offers to clear trust. A widened folder or file mode is narrowed back to owner-only, and staged `.staging-*-part` copies left by a crash are removed from that folder at startup and on re-enrollment. This backend relies on file permissions alone: keep the disk encrypted. A provider that is registered but fails to start, or that never claims the standard name, is not "absent": that is WalletLocked, not the file. At enrollment the user may still choose the file explicitly (next bullet).
- **Explicit file choice at enrollment (R85).** Secret Service may be present but unusable while enrolling a device that has no key yet. That covers a provider that fails to start or never claims the standard name, a session bus that exists but cannot be connected, a provider with no default collection, an unlock prompt that is dismissed, times out or fails, and a provider that does not keep the new secret. The wallet screen then keeps **Retry after unlock** (unchanged behaviour) and also offers **Store the key in a protected file instead**, with "Protected by file permissions; keep your disk encrypted".
  - Choosing it records the file backend exactly as `PORCHLIGHT_KEY_STORE=file` does, then returns to the enrollment screen, which names the chosen store. Re-enter the URI and password; they are never persisted.
  - Nothing is recorded until the user chooses, and Retry withdraws the offer.
  - A device that already has a key never sees the choice: an enrolled Secret Service device keeps WalletLocked, and switching means re-enrolling. The key store also refuses the choice once `device.p12` exists.
  - Enrollment's **Start over** deletes a `device.p12` that never enrolled, rather than leaving an empty one. The choice therefore stays reachable after Start over, and startup does not touch the wallet for a device with no key. The backend record is kept.
- **Recorded, never migrated.** `<data>/porchlight/device-key/backend` is a non-secret device setting (`secret-service` or `file`, mode 0600). It is written at enrollment once the secret is durable and honoured on every start. A Secret Service device whose provider later disappears or stays locked gets WalletLocked, never the file. A file device keeps the file even after a provider appears. Devices enrolled before this change have no record and are Secret Service devices. A `device.p12` with a key file but no record is a file device that lost its record: LocalTrustLost, never a Secret Service prompt. An unrecognised record is LocalTrustLost (re-enroll). **To switch backends, re-enroll:** Clear trust / Enroll again / self-revoke deletes `device.p12`, the key file and the record, and the next enrollment chooses again. Secret Service items stay in the wallet and are reused if the device re-enrolls there.
- **Settings → Account and devices** shows the backend: "This device's key: Secret Service (your desktop wallet)", or "This device's key: owner-only key file" plus "Protected by file permissions; keep your disk encrypted".
- **Override (explicit opt-in).** `PORCHLIGHT_KEY_STORE=file` in the app's environment at enrollment forces the file backend without probing D-Bus. It is meant for tests, development and disposable gate profiles (the M5 gate), or a machine whose Secret Service cannot be used. It is ignored once a backend is recorded, and any other value refuses enrollment rather than guessing. Leave it unset for normal use.

## Secret Service activation on niri (R24)

The desktop checks the session bus before reporting an unavailable wallet. On an unavailable Secret Service it calls D-Bus `StartServiceByName("org.freedesktop.secrets", 0)`. Only if the bus reports that name is not activatable (`ServiceUnknown`) does it call `StartServiceByName("org.kde.secretservicecompat", 0)` as a harmless extra nudge for KDE's ksecretd, then retries the collection check once. Other activation errors fail closed. For a Secret Service device this never creates a collection or switches to the file. At enrollment the same single attempt decides whether a provider exists (previous section).

On a desktop without a built-in keyring (for example a standalone compositor such as **niri**), any Secret Service provider in the user's D-Bus session works: GNOME Keyring, KeePassXC with Secret Service integration, or KDE's **ksecretd** (activation name `org.kde.secretservicecompat`, exporting `org.freedesktop.secrets`; older distributions may use kwalletd6/secretservicecompat, so inspect the installed D-Bus activation registration rather than guessing a daemon path). Without a provider, Porchlight enrolls with the owner-only key file. Installing a provider later does not move an enrolled device; re-enroll to switch. Ensure the desktop starts inside the same user D-Bus session. Do not substitute systemd service commands or start another user's wallet daemon.

Controller checks: with no Secret Service currently running but one activatable, launch Porchlight and confirm standard activation (KDE nudge where needed) and encrypted PKCS#12 reuse. With the default collection locked, the provider's unlock prompt must appear at startup. Dismiss it: the gate stays on wallet recovery and no `device-key/pkcs12-password` appears. **Retry after unlock** and complete the prompt to restore. On a session with no provider at all, enroll a disposable profile and confirm the file backend. No printed wallet or key-file secrets are permitted. Fake-bus tests cover activation order, the one retry, unlock prompts and backend selection. Native provider prompts and dbus-java exception mapping need this live check.

## Offline gate

```bash
umask 022
. scripts/env.sh
(cd client && ./gradlew --offline --console=plain --max-workers=4 -Pkotlin.incremental=false :shared:jvmTest :shared:testAndroidHostTest :contract-check:jvmTest :androidApp:assembleDebug :androidApp:assembleDebugAndroidTest :desktopApp:jar :androidApp:lintDebug)
(cd client && ./gradlew --offline --console=plain --max-workers=4 -Pkotlin.incremental=false :androidApp:verifyManagedDeviceFilter)
CARGO_NET_OFFLINE=true scripts/ci.sh --sandbox
```

Real OkHttp/Ktor tests against a local TLS 1.3 SSLServerSocket with generated chains reject wrong roots, absent pinned roots, leaf pins, wrong names and expired leaves. Fake-wallet tests exercise encrypted PKCS#12 persistence, pending-attempt restart/multiple-token retention/atomic removal and unlock retry. Fake-Secret-Service tests (`DesktopKeyBackendTest`, no real D-Bus) cover:
- backend selection and recording, including a late KDE compat name and a session bus that exists but cannot be used;
- the unlock prompt, its dismissal, and the one-prompt-per-attempt rule;
- the `CreateItem` read-back;
- the `PORCHLIGHT_KEY_STORE` override and the R85 choice (including after Start over);
- pending attempts per backend, and re-enrollment switching;
- staging cleanup, and symlinked/foreign-owned/non-POSIX key folders failing closed.

`SecretServiceDbusIntegrationTest` drives the production dbus-java adapter against an in-process provider on a private `dbus-daemon` (never the session bus or a real wallet): Unlock, Prompt, Completed, Dismiss on timeout, and the ServiceUnknown mapping. Its daemon start is bounded, and it is skipped only where no `dbus-daemon` can run, such as some sandboxes. Fault-injection tests exercise committed-but-lost enrollment replies, storage failures before sending and before trust commit; virtual-time tests cover same-session renewal retries and threshold crossing while PIN-locked. Common tests cover wire vectors, headers, failover, problem decoding, auth/gate transitions, reset normalization and renewal boundaries; Compose tests cover semantics, minimum targets, double font scale and no restored content before PIN.

## Controller: real desktop + Secret Service

Run on the main tree after merging, with the dev stack and an unlocked session wallet (any Secret Service provider; the live probe exercises the Secret Service adapters, not the file backend). Do not run host and sandbox builds against the same Gradle home concurrently.

```bash
umask 022
. scripts/env.sh
scripts/dev-stack.sh up
PORCHLIGHT_E2E_REQUIRED=1 scripts/c1b-desktop-e2e.sh
```

The helper obtains a fresh URI via `porch --json enroll` into a private `var/trust-e2e` file, asks for the dev password without echo (or consumes private `PORCHLIGHT_E2E_PASSWORD`), then runs exactly:

```bash
cd client
PORCHLIGHT_E2E_REQUIRED=1 ./gradlew --offline --console=plain --max-workers=4 -Pkotlin.incremental=false --rerun-tasks :shared:jvmTest --tests 'app.porchlight.shared.trust.DesktopE2eTest'
```

It sets `PORCHLIGHT_E2E_URI`, `PORCHLIGHT_E2E_PASSWORD`, `PORCHLIGHT_KEYRING_TEST=1` and **`PORCHLIGHT_E2E_REQUIRED=1`**, without printing them. Required mode fails rather than skipping on missing/blank configuration. With no URI/password, desktop e2e is explicitly skipped; the wallet probe separately requires `PORCHLIGHT_KEYRING_TEST=1`.

**Renewal (R22):** the live probe drives `refreshEndpointsAndRenew(forceRenewal = true)` over production mTLS and asserts **409 `renewal-too-early`** for the freshly issued certificate, with no change to its stored certificate. No dev database fixture exists and immutable certificate history is never edited. The local TLS-server test issues an eligible 15-day certificate and a renewed certificate from its test CA, exercising real renewal, same-key CSR, atomic PKCS#12 replacement, reopening and subsequent mTLS reuse. The controller may clean up test devices and the three real wallet items under service `app.porchlight.test` after the live run; local test PKCS#12 and temporary input files are removed.

Also lock the wallet and relaunch a Secret Service-enrolled app. The provider's unlock prompt must appear. Dismiss it: the gate shows unlock/retry and no `device-key/pkcs12-password` appears. Retry and complete the prompt to restore. Then, with a disposable `XDG_DATA_HOME` and either no provider or `PORCHLIGHT_KEY_STORE=file`, enroll. Settings → Account and devices must name the owner-only key file with the disk-encryption note, with `device-key/` at 0700 and `pkcs12-password` at 0600.

## Controller: Android managed devices + real emulator enrollment

```bash
umask 022
. scripts/env.sh
(cd client && ./gradlew --offline --console=plain --max-workers=4 -Pkotlin.incremental=false :androidApp:phonesGroupDebugAndroidTest)
```

This runs deterministic shell/gate/relock/recreation/insets and AndroidKeyStore generation/P-256/non-exportability/non-auth-bound/CSR + TLS-style NONEwithECDSA digest-signature and missing-Keystore transfer checks, encrypted pending-attempt reopen/recovery (lost device/metadata keys, malformed ciphertext, wrong key alias), plus lifecycle-driven secure-window checks on API 35 and API 36. The debug-only `ShellFixtureActivity` calls shared Android test support with fake storage/transport; production entry points never call it, and the shared fixture rejects non-debuggable apps.

`@ControllerOnly` excludes the live enrollment class via the configured `notAnnotation` runner argument; it is **not** an assumption skip (UTP can count those as failures). `verifyManagedDeviceFilter` is a prerequisite of both device tasks and the group. Run its positive assertion and this expected-failure check without starting emulators:

```bash
(cd client && ./gradlew --offline --console=plain --max-workers=4 -Pkotlin.incremental=false :androidApp:verifyManagedDeviceFilter)
# EXPECTED FAILURE: Managed-device runs must exclude @ControllerOnly
(cd client && ./gradlew --offline --console=plain --max-workers=4 -Pkotlin.incremental=false :androidApp:verifyManagedDeviceFilter -Pandroid.testInstrumentationRunnerArguments.notAnnotation=org.junit.Ignore)
(cd client && ./gradlew --offline --console=plain --max-workers=4 -Pkotlin.incremental=false :androidApp:phonesGroupDebugAndroidTest --dry-run)
```

Shared applies the standalone `com.android.lint` plugin from the locked AGP 9.4.1, with Android lint configured to abort on errors. CI runs `:shared:lintAndroidMain` as well as app dependency lint. A removed API-31 guard now fails shared lint with `NewApi`; the guarded source passes. Manually verify exact-alarm recovery on API31/32 (managed devices cover only 35/36).

For real enrollment against the dev listener (an already booted `porch-api35`/`porch-api36` AVD; select its actual adb serial):

```bash
umask 022
. scripts/env.sh
scripts/dev-stack.sh up
PORCHLIGHT_E2E_REQUIRED=1 scripts/c1b-emulator-e2e.sh emulator-5554
# With the API 36 AVD booted at its selected serial:
PORCHLIGHT_E2E_REQUIRED=1 scripts/c1b-emulator-e2e.sh emulator-5556
```

The helper recreates just the dev API with **`ENROLL_HOST=10.0.2.2`**, installs the debug APK, obtains a new URI, and transfers URI/password only over stdin into `run-as app.porchlight` private storage (not command arguments). An EXIT trap recreates the API with the original `ENROLL_HOST` (or `var/dev.env` value if unset), even after a failed test. It exports required mode and passes its non-secret flag explicitly to instrumentation. It invokes:

```bash
cd client
PORCHLIGHT_E2E_REQUIRED=1 ANDROID_SERIAL=emulator-5554 ./gradlew --offline --console=plain --max-workers=4 -Pkotlin.incremental=false :androidApp:connectedDebugAndroidTest -Pandroid.testInstrumentationRunnerArguments.PORCHLIGHT_E2E_REQUIRED=1 -Pandroid.testInstrumentationRunnerArguments.notAnnotation=org.junit.Ignore -Pandroid.testInstrumentationRunnerArguments.class=app.porchlight.android.trust.AndroidEnrollmentE2eTest
```

The helper overrides `notAnnotation` with the harmless `org.junit.Ignore` sentinel only for this selected connected live class; never remove the `ControllerOnly` exclusion for a managed-device suite. The controller must confirm the annotated class actually executes, not a zero-test run.

The test generates a separate Android Keystore identity, enrolls with an injected lost reply after the real server commit, reopens the real encrypted store/controller and replays the saved key/CSR, checks the required PIN state, logs in, calls `/v1/me` and `/v1/endpoints`, and logs out. It does not display restored app content and therefore does not authenticate a real Android PIN; the following UI checks remain controller checks after C2 wiring:

- Paste URI and scan QR (emulator virtual-scene QR); deny camera once, then use recovery or paste.
- On launch/background/resume, cancel device credential: restored/private content must never appear. Configure a secure emulator lock and succeed with **device credential only**, never biometric/password fallback in the app.
- Deny notifications, battery exemption and any unavailable exact-alarm capability; verify explicit recovery links and retry. Grant them and verify content is reached; also verify “Continue with limited reminders” prevents settings/OEM restrictions from dead-ending onboarding.
- Use 200% font scale; scroll every form and verify labeled/tappable targets. Exercise reset, expired session, wrong credentials, password change, bound revoke/self-revoke, logout retry, upgrade and re-enrollment screens.

The development emulator hint is shown only for Android debug builds, using the application debuggable flag. Android release and desktop builds never show it.

No emulator, external network, live Secret Service or real-stack checks run inside the implementer's sandbox.

## Controller: R78 enrollment recovery and task snapshots

- On API 35/36, unlock a private item and open the attachment photo picker half-sheet, camera permission dialog, and file picker. Enter Overview while each is over our content: the Porchlight card must be blank/redacted. Repeat after rotation and helper/credential transitions. Foreground screenshots must still work after resume; ON_STOP re-entry must require credentials. API 30–32 capture timing remains a separate controller check if an image becomes available.
- Exercise an in-progress enrollment retry followed by commit, account lockout followed by successful retry, offline/handshake failure with corrected password, and committed lost reply with original/different password. Different credentials must reach the server under the stored key, receive key-reuse if processed, and never be blocked locally. **Start over** must clear only pending local state/unused key and send no server request.
- Run the annotated real-enrollment test above on both APIs, and additionally force-stop/relaunch after a committed dropped reply. Check same-device replay before the nightly expired-token purge. After the actual purge, retry must report `enrollment-token-invalid`, clear pending state, and allow a fresh admin URI; do not assume 24 hours of replay. Do not print tokens/passwords/keys.
- On Android and desktop, suspend across the renewal threshold, foreground/unlock, and verify overdue renewal without resetting a pending failure backoff. Fail near expiry to check the warning; recover to clear it. Verify an unchanged refresh does not close live calls/downloads, and a hung refresh does not block logout/account actions. A shortened server lifetime returning `renewal-too-early` should stay on daily cadence.
