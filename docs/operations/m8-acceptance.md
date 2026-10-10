# M8 laptop acceptance (controller only)

Requires Docker/Compose, signing/release tools, emulator porch-api35, curl, age-keygen, and a separately held
recovery identity under var. No production server, Funnel or physical phone. Stop all emulators first: the gate refuses
an active one, reaps a stale netsimd before boot, then stops its emulator and reaps netsimd again on exit.

Produce **two genuinely distinct increasing signed releases** using scripts/release.sh (see release.md/O1 report).
The old one should be a clean release containing O2b's smoke hook; the newer one must have a strictly larger version
code, not the same directory with a changed manifest. Keep dist immutable. Do not regenerate existing signing keys.

```bash
umask 022
. scripts/env.sh
deploy/scripts/m8-acceptance.sh --source dist/0.1.0 --upgrade-source dist/0.1.1 \
  --identity var/backup-recovery.key --keep
# Use the actual old/new release versions; omit --keep for successful scratch teardown.
```

Gate sequence: signed clean server install → porch pinned TLS enrollment → item/list/attachment/comment/member
export/household export with complete.json + gzip trailer/inventory verification → immediate backup with SHA-256
receipt → independent restore drill (manifest/member hashes, table counts/migrations/CA before writers) → newer
upgrade and preserved item/list/comment/blob/export checks → desktop install into scratch HOME → real production
pull engine over scratch mTLS, compare local bundle hash to server bytes → signed APK install on porch-api35.
The backup receipt is checked against the server-recorded SHA-256 **and size**, not a newly computed local hash.
Desktop installation scratches HOME and XDG data/config/state directories. The gate verifies the release APK
signature, **uninstalls any existing `app.porchlight` on porch-api35 (destroying that AVD's development app data)**,
installs the signed release, pulls the installed APK and verifies that its signer matches the release artifact.
The desktop smoke validates the full SHA against server metadata before publishing. No wallet or regular installed
app state is used by this headless gate; interactive Secret Service/normal lifecycle checks remain controller checks
in client-trust.md. An API35 install is not a claim that all Android UI/emulator tests passed.

Defaults: scratch loopback PG56432, LAN19443, enroll19444; emulator5580; networks172.28.242/243. Override ports with
M8_PG_PORT/M8_LAN_PORT/M8_ENROLL_PORT and networks with M8_BACKEND_SUBNET/M8_INGRESS_SUBNET. The independent drill
has its separate DRILL_* overrides and refuses an existing var/restore-drill directory. Run gates serially.

Only fixed PASS/FAIL lines on stdout. Secrets/credentials/plaintext/decrypted state/logs remain in private
var/m8-acceptance.XXXXXXXX and (with --keep) var/restore-drill. --keep leaves scratch Docker stacks/HOME/credentials
for inspection (including the release APK on the AVD), but always shuts down the owned emulator. Without --keep,
once release installation is attempted, cleanup uninstalls `app.porchlight` **before** emulator shutdown on success,
failure or interruption, so later cold-boot debug installs do not conflict with the release signer. An uninstall
failure makes the gate fail and retains private diagnostics (`apk-cleanup.log`); manually uninstall before debug work.
Successful runs tear down/remove scratch state; failures preserve private diagnostics after stack teardown. Never enable shell tracing or print these logs
wholesale. Inspect any failed stage, deliberately tear down kept stacks/volumes, then remove private roots (postgres
bind files may require the verified backup image as root, as in the gate cleanup). The recovery identity is never
removed. Standalone restore-drill is an independent PASS/FAIL gate, not a reused main scratch root.

Offline CI covers syntax/contract hooks + deterministic archive corruption fixtures + fake-server desktop pull
logic. **Only the controller can claim the live M8 gate passed.**
