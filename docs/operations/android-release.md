# Android shrinking and release smoke

`release` uses R8, resource shrinking, the optimized Android defaults and `androidApp/proguard-rules.pro`.
Signing is unchanged: `scripts/release.sh` supplies the owner's keystore settings; without them `assembleRelease`
produces an unsigned APK. Debug is not minified and keeps its existing fixture suite. Never ship the `minified` APK.

## Retention boundaries

- kotlinx.serialization's bundled consumer rules preserve generated model companions/serializers, singleton
  serializers and annotations under R8 full mode. AppRoute's typed routes use ordinary, unnamed companions.
  Project rules retain signatures/inner-class metadata; there is no keep-all model or serializer rule.
- AAPT's merged-manifest rules preserve Application, activities, receivers, services and providers. WorkManager's
  consumer rules preserve live ListenableWorker subclass names and their constructors. Both application workers
  are checked in the release seeds and by name/constructor lookup in the minified runtime smoke.
- SQLDelight's schema, queries and Android driver are direct calls. No package-wide keep is needed.
- OkHttp uses the packaged `assets/PublicSuffixDatabase.list` on Android, with its own consumer rules for optional
  platforms. Compose's four fonts, drawable and font licenses remain assets, not Android resource-name lookups.
- BC remains a dependency for ASN.1/PKCS#10 construction. CertificateTools uses direct calls and platform JCA
  signing, not BouncyCastleProvider registration; no provider-wide keep or additional warning suppression is needed.
  The smoke constructs a CSR without enrollment or network. Adding a provider later requires reviewing its
  reflective service registration and this rule set.
- `deploy/tests/audit-apk.py` checks shrinker reports, dex growth, worker constructor seeds, fixture exclusion,
  fonts/drawable, font/BC notices and the suffix database. It also checks release/JVM runtime dependency locks:
  Ktor content negotiation/JSON integration is test-only; SQLDelight coroutine extensions are unused and removed.

## Why a separate minified smoke variant?

`minified` inherits release's R8/resource configuration and non-debuggable setting, but uses the existing debug
signing key and `app.porchlight.minified` application ID, so it cannot replace an enrolled production install.
`-Pporchlight.minifiedSmoke=true` selects its isolated instrumentation sources instead of the debug fixture tests.

Instrumentation is black-box: it launches the real MainActivity and checks the PIN gate through Android
accessibility. Then it starts a non-exported, **minified-only** probe activity for generated-model and typed-route
serializer lookup/round-trip, the application's SQLDelight open/query, both WorkManager constructors, PKCS#10
construction and all four fonts. The probe is optimized together with the application. Instrumentation must not
link directly against pre-R8 application methods: R8 can inline/remove them even when their behavior remains in
production. There is no credential bypass. The probe roots these representative paths only in `minified`;
release excludes it entirely. It reports stages/errors, never keys or enrollment values.

`proguard-minified-test.pro` applies **only** to `minified`: AGP omits shared app dependencies from the test APK,
while app R8 cannot see their instrumentation callers. The extra rules retain just those cross-APK members:
AndroidX tracing (runner startup), concurrent-futures/lifecycle and Guava's future interface (test core), and
Kotlin/coroutine entry points used by test core and the smoke. Facade/superclass lookup and descriptor types are
preserved without package-wide keeps. The shipping release has none of these extra roots. The offline
`deploy/tests/audit-apk-fix1.py` checks the built test APK's app-member references against the target dex and
checks release-rule isolation. Dependency upgrades require rechecking this ABI and the emulator smoke.

Fast/sandbox CI builds/lints both APKs and compiles the smoke; full CI runs both API 35/36 managed-device suites.
The smoke requires a fresh managed-device app with no enrollment or device credential; it never sets a PIN or
unlocks a device. It is not proof of enrollment, renewal, real TLS or notification delivery.

## Controller-only emulator commands

From the repository root, in **bash**, with cached AOSP API 35/36 images:

```bash
bash scripts/test-managed-clients.sh --minified
```

Use this wrapper rather than bare Gradle managed-device tasks: UTP can report `BUILD SUCCESSFUL` after an
instrumentation startup crash with zero tests. The wrapper clears stale results, propagates Gradle failures and
requires fresh nonempty, complete, passing XML for **each** of API 35 and 36. Missing/malformed results, zero-test
suites, all-skipped suites, failures/errors and nonzero UTP exit codes fail the gate. Offline contracts exercise
both variants, including a successful Gradle process with zero tests and a crash leaving only stale XML.

`bash scripts/test-managed-clients.sh` runs the unchanged debug suite first and this smoke second. Run
`bash scripts/ci.sh --full` for all controller gates. Before shipping, also verify a release-signed build on both
APIs: enrollment → PIN unlock → renewal → process restart with persisted keys/cache; typed navigation and fonts;
calendar/file pick/upload/download/update install; notifications/actions and background WorkManager. Those paths
need a local test household and are not covered by this enrollment-free smoke. Production signing stays owner-run.

Offline build-only checks, no emulator:

```bash
. scripts/env.sh
(cd client && ./gradlew --offline --max-workers=4 --console=plain -Pkotlin.incremental=false :androidApp:assembleRelease)
python3 -I deploy/tests/audit-apk.py
(cd client && ./gradlew --offline --max-workers=4 --console=plain -Pkotlin.incremental=false \
  -Pporchlight.minifiedSmoke=true :androidApp:assembleDebug :androidApp:assembleMinified :androidApp:assembleMinifiedAndroidTest :androidApp:lintMinified)
python3 -I deploy/tests/audit-apk-fix1.py
python3 -I deploy/tests/audit-managed-results.py
```
