# Build a signed release (laptop only)

Run from the source root. Docker/Compose, the pre-cached Gradle dependencies, JDK 25/Android SDK and Docker-backed
`age`/`minisign` wrappers are prerequisites. Build and sign locally; the owner can then publish to GitHub Releases
with the separate command below. Docker builds may fetch pinned base images/packages; the release build is an
operator gate, not a sandbox test.

```bash
umask 022
. scripts/env.sh
scripts/release-keys.sh             # exactly once; refuses an existing/partial var/release
scripts/release.sh
```

`VERSION` is plain MAJOR.MINOR.PATCH; minor and patch are 0–99. Android versionCode is MAJOR*10000+MINOR*100+PATCH
(positive and at most 2100000000). The production alias is `porchlight`. Signing reads `PORCHLIGHT_KEYSTORE` and
`PORCHLIGHT_KEYSTORE_PASSWORD_FILE` (environment or Gradle properties); both absent intentionally builds unsigned
CI APKs. Signed builds require `--no-configuration-cache`, enforced before reading the password and set automatically
by release.sh. `release.sh` requires keys, a signed APK and successful `apksigner verify --print-certs`; it never ships
an unsigned APK. Before building, the last line of `var/release/minisign.pub` must match the committed
`deploy/release-keys/minisign.pub`. After building, the APK's full signer-certificate SHA-256 set must match
`deploy/release-keys/android-signer.sha256` (comments/blank lines ignored). A different minisign identity or APK
signer is refused; changing either public anchor is a deliberate trust change, not an automatic recovery step.
See [key custody](keys.md).

`dist/<version>/` contains image saves for API and backup, the musl `porch` extracted from the Docker builder's
`cli-export` stage, a Linux desktop app image with a bundled runtime, APK, pinned-key installer, compose, example
configuration, dormant digest-only Funnel override, icon, helper library, Postgres bootstrap, migration checksums,
`release.json`, JSON manifest and minisign signature. No Python bytecode or `__pycache__` directories are packaged.
`release.json` is a signed manifest member recording version, git commit/dirty status, the machine profile's
`PORCHLIGHT_RELEASE_REPO` as `source.github`, APK package/versionCode/signer set, and a sorted `desktop.files` list
(including `release.json`) for subset downloads. Missing git/commit or failed git queries record `commit: null` and
`dirty: true`; unset/empty repository records `source.github: null`. A configured slug must be `owner/repo` (owner
1–39 alphanumeric/hyphen characters, starting alphanumeric; repository 1–100 alphanumeric/dot/underscore/hyphen
characters). Invalid slugs are refused without printing their value. Local builds without provenance remain usable,
but cannot be published.
Every payload file (including metadata, helpers and bootstrap) has a size and SHA-256 in the signed manifest. The script verifies
its own output. It refuses any existing version directory unless `--force`. Preserve published artifacts externally:
`--force` is for a failed/local build, never for mutating an already distributed release.

SQLx checks applied migration checksums during migrate. `release.sh` also compares migrations against every prior
verified local release's signed `migrations.json` before rebuilding, including `--force`. Keep prior distributions so
this source-side immutability guard and rollback installers remain available. Never edit released migrations.

## Publish

The **owner**, never CI, runs `scripts/publish-release.sh <V>` from a git checkout. Prerequisites: `gh auth login`,
`PORCHLIGHT_RELEASE_REPO` set in the gitignored `.env` before building, and main committed and pushed to origin.
Use the usual `scripts/env.sh` toolchain; `apksigner` can be on PATH or in the configured Android SDK. Recommend
turning on GitHub **immutable releases** in the repository settings before publishing.

```bash
. scripts/env.sh
PORCHLIGHT_PUBLISH_DRY_RUN=1 scripts/publish-release.sh 0.1.6
scripts/publish-release.sh 0.1.6
```

The dry run performs every read-only git/GitHub query and verification, builds the tar and notes under
`var/publish/<V>/`, then prints the exact shell-quoted `gh release create` argv. It creates no GitHub release and
downloads nothing. Both modes refuse:
- a missing/modified dist, invalid manifest signature, or APK signer differing from the committed pin or signed
  `release.json` signer set;
- a dirty working tree, a different `VERSION` at HEAD, a missing/different recorded commit or a dirty release build;
- HEAD not contained in remote main (push main first);
- a missing source repository, or a mismatch among `.env`, the signed metadata and `gh repo view`;
- an existing `v<V>` release, or a version not above every existing `vX.Y.Z` release (up to 1000 queried).

Five flat assets are uploaded: `manifest.json`, `manifest.json.minisig`, `porchlight-<V>-dist.tar`,
`porchlight-<V>.apk`, and `install.sh`. The deterministic tar has sorted relative paths without `./`, zero mtime and
numeric owner/group zero: every manifest member plus the manifest/signature. Notes contain version, commit,
APK SHA-256, APK signer digest(s), the decoded minisign public key's SHA-256 fingerprint and verification guidance.
No private signing material is uploaded.

After creation the script downloads the assets under `var/publish/<V>/check/`, compares the standalone assets,
safely extracts the tar, verifies its signature and every member, and checks that it is the same signed manifest.
Only then does it print `PASS published v<V>`. A failure here does **not** delete or replace the uploaded release;
inspect it before distributing, and never bypass the refusal to overwrite an existing version.

## Verification

Tests: `scripts/ci.sh --sandbox`; `. scripts/env.sh && cd client && ./gradlew --offline --max-workers=4 --console=plain
:androidApp:assembleRelease` (unsigned without production settings). Real signatures, images, desktop launch and
signed APK installation are separate controller gates. See [keys](keys.md), [install](install.md), [upgrade](upgrade.md).

The generated installer embeds its bash and Python verification helpers; sibling `lib/` is signed data, not a second
trust root. Desktop archives use `tar --hard-dereference -chzf` to materialize repeated jlink symlink targets as regular
files (executable modes preserved). The fixture gate installs the exact distribution it built, not a hand-made archive.

Runtime modules: `:desktopApp:suggestModules` succeeded offline and suggested `java.instrument`/`jdk.unsupported`.
These plus reflection-loaded TLS provider `jdk.crypto.ec` are explicit; Compose adds its desktop baseline. The measured
JDK 25 runtime is **80 MiB**, down from **164 MiB** with includeAllModules. The whole JDK is no longer bundled; verify
graphical startup/keyring/tray after C1a/C4 merge and re-run discovery when desktop dependencies change.

The shipped desktop image is `:desktopApp:createReleaseDistributable` (`build/compose/binaries/main-release`): ProGuard
7.8.0, shrink only (no obfuscation/optimization), rules in `client/desktopApp/proguard-rules.pro`. Reflection-, JNI- and
ServiceLoader-driven libraries and the application are kept whole; Compose and BouncyCastle are shrunk (0.1.7 tarball
70 MB, 0.1.6 103.5 MB). A post-step removes stale upstream jar signatures and service lines naming removed classes.
`deploy/tests/audit-desktop.py` checks the image (no signatures, every service class shipped, no `invokespecial` to a
non-direct superinterface in any class, SQLite resources intact) and `:shared:packagedDesktopSmoke` runs trust, cache,
launch and D-Bus tests on the shrunk jars with the bundled JVM. After a desktop dependency change, rebuild, rerun both and
launch the image once in a scratch profile; ProGuard warnings about references missing from the classpath usually mean
a library built against another version of a dependency (that is how java-keyring's dbus-java 4 binding was found).
`createDistributable` (unshrunk) remains for development and demos.
