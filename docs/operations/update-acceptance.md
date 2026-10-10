# Remote-update acceptance (controller only)

Run from the **main tree on the laptop**, after the normal offline/fast CI checks. Never run these gates on a server.
They require Docker, cached Gradle dependencies, JDK/Android SDK from `scripts/env.sh`, the `porch-api35` AOSP AVD,
and the existing debug keystore at `$HOME/.android/debug.keystore`. Close all emulators first. The gate deliberately
uninstalls Porchlight and its test package from this development AVD; do not use an AVD containing wanted app data.
The device must be at an unlocked lockscreen; the gate never changes somebody's device credential.

```bash
bash deploy/scripts/update-gate-build.sh
bash deploy/scripts/update-acceptance.sh
```

Budget roughly 30–90 minutes for three builds (cold Docker builds can take longer), then 5–15 minutes for acceptance.
Allow tens of GiB of free space: image archives, the build copy, local mirror and installed releases coexist.
There is no network publication. Never publish or deploy these scratch releases: **9.9.10 intentionally cannot migrate**,
and all three APKs use the public Android debug signing identity.

## Build boundaries

The builder refuses an existing `updgate-build` tree or existing build artifacts. It uses `scripts/wt.sh create
updgate-build`, creates throwaway minisign/Android keys only inside that copy, then replaces the copy's APK key with
an import of the debug certificate. Both copy pins are rewritten. The debug app and release app therefore have the
same signer, permitting a real Android package replacement while retaining instrumentation/run-as at 9.9.8.
The copy's debug signing configuration is explicitly pointed at that same keystore.

It builds 9.9.8, 9.9.9 and the failing-migration 9.9.10, updating only that copy's VERSION, Cargo workspace/lock and
migration inventory. The main tree's version, signing keys, pins and `dist/` are untouched. Signed dists and the
9.9.8 debug/test APKs go to owner-only `var/update-gate/{dists,android}/`. `build.json` records every copied artifact's
size/SHA-256, versions and public trust anchors. Acceptance verifies this inventory and each signed manifest first.

`--keep` preserves the build copy for investigation; otherwise even a failed build drops it. Private build logs stay
under `var/update-gate/build-*/`. A failed build has no completion record and cannot be accepted. Inspect/remove its
partial `dists/` and `android/` before retrying. Use `scripts/wt.sh drop updgate-build` to drop an explicitly kept copy.

## What the attestations prove

- `PASS scratch release … built` / `PASS update gate build`: three real signed builds, debug/test APK signer equality,
  and the complete artifact inventory. Not a deployment or acceptance pass.
- `PASS local signed release mirror`: the publish script's deterministic tar layout served on a free **loopback** port;
  latest initially resolves to 9.9.9. Only the installer test override permits HTTP.
- `PASS scratch server 9.9.8 …`: a separate `porchlight-update-gate-*` Compose project, root, agent socket, generated
  credentials and recovery identity; desktop/probe devices enrolled through `porch dev-init`. Ports are dynamically
  selected; default subnets are distinct from the other gates. Optional `UPDATE_GATE_BACKEND_SUBNET` and
  `UPDATE_GATE_INGRESS_SUBNET` must be unused laptop-local Docker subnets.
- `PASS client-release hashes and 426 recovery exemption`: metadata equals the signed dist's APK and desktop inventory,
  APK bytes/ETag match, and a sessionless mTLS client below the configured minimum can discover/download its update
  while `/v1/me` still returns 426. These assertions run again after upgrade and rollback.
- `PASS upgrade 9.9.9 …`: the installed trusted wrapper fetched from the mirror, upgraded, retained an item, advertises
  9.9.9 and is healthy. No downloaded incoming payload or plaintext rollback dump remains. The installer currently
  leaves an **empty** `incoming/` parent; the gate accepts that, not any leftover payload.
- `PASS already at 9.9.9 and refused downgrade`: the same-version wrapper is a no-op, and 9.9.8 is rejected with exit 2.
- `PASS automatic rollback …`: latest switched to 9.9.10; the forced SQL exception was observed in the migration log,
  the command exited 1 with `ROLLED BACK to 9.9.9`, and API/data/advertisement recovered without a plaintext dump.
  The installer intentionally retains the failed incoming release and diagnostics until gate cleanup.
- `PASS desktop …`: mTLS-advertised desktop subset downloaded with safe paths and checked hashes, installed through
  the **old trusted** installer in scratch HOME/XDG directories, current/previous links checked, a one-byte corruption
  refused, and `porchlight-update --rollback` checked. This does **not** attest a visible desktop restart or UI click.
- `PASS Android real … signers`: device PackageManager reads actual certificate sets; preflight rejects an advertised
  foreign key, and a separately signed real APK asset is rejected with the different-key message. The installed
  app's own archive passes signer checks. The asset is built offline for androidTest; private keys are transient in
  `var/update-signer-fixture/`, never in the APK/build assets. This test also runs in managed-device full CI.
- `PASS Android Available(9.9.9) …`: the real UpdateController enrolls over pinned TLS, discovers 9.9.9, downloads,
  verifies and commits a PackageInstaller session, and instrumentation observes system confirmation before returning.
  The existing debug-only test activity supplies a foreground window without unlocking household content or opening
  a device-PIN dialog over the installer; banner interaction is covered separately by UI tests. Enrollment secrets
  travel in private JSON via run-as stdin, never adb argv. `adb reverse` keeps the scratch
  localhost SAN/endpoints valid on the emulator without rewriting enrollment trust.
- `PASS Android system-confirmed …`: the host finds the unique enabled system Install/Update button by resource ID,
  package, text and observed bounds, taps its center, then observes `versionName=9.9.9` within 120 seconds. No `adb
  install` substitutes for this update.
- `PASS update acceptance`: all checks **and cleanup** finished successfully. Earlier PASS lines alone are not enough.

## Cleanup and evidence

Both entry points hold `var/update-gate/.lock`, preventing concurrent runs. Acceptance uses
`var/update-gate/run-<UTC timestamp>.<random>/` (0700), a distinct emulator console port, and always tears down its
Compose project and stops only the emulator/mirror it started. It removes both APK packages before stopping the
emulator and uses the shared stale-netsimd helper. It never operates on physical devices or the real desktop install.

Private `.log` and confirmation `.xml` evidence survives normal cleanup. Scratch stack/data/keys, downloaded files and
HOME/XDG installs are removed, including container-owned pgdata using only that scratch bind mount. `--keep` preserves
these directories for debugging **but still stops the stack, mirror and emulator**. If cleanup fails, the final result
is FAIL and state remains for inspection. Treat every retained run directory as sensitive; never paste its contents.
Built dists remain for repeat acceptance; after inspection, delete `var/update-gate/` to reclaim them. Docker image
cleanup, if desired, should name only the gate's `porchlight-api`/`porchlight-backup` tags 9.9.8, 9.9.9 and 9.9.10.

The offline `deploy/tests/update-gate-contract.py` is wired beside the M8 contract in `scripts/ci.sh`. It checks shell
syntax, isolation/cleanup hooks, generated signer-test wiring and driver path/checksum/version/button decisions.
It neither launches these gates nor claims an Android or Docker pass.
