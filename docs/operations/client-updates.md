# In-app client updates

Enrolled clients check their household server at startup, on resume (at most hourly), and when the live stream
reconnects. They never contact GitHub or update past the server's advertised release. Development versions without a
numeric `VERSION_CODE` cannot update. Checks do not install anything automatically.

- **Update banner:** choose Update, or Later to hide that version's banner on this device. A newer version appears
  again. Settings → About remains available after dismissal, with app/server versions and Check for updates.
- **Super admins:** About also shows a newer signed server release and the command to run on the server. The command
  is selectable/copyable text, not an execution button. Other members never request server-release status.
- **Upgrade required (426):** Update now uses the same updater. Its requests authenticate the enrolled device over
  pinned mTLS without a member session. If the server advertises no release, ask an admin to upgrade the server.

Downloads use the existing LAN/Funnel transport, a ranged checkpoint and If-Range. A retry rehashes the checkpoint;
ETag, lengths, range and the complete SHA-256 must all match. A corrupt completed download is deleted and is never
passed to an installer. Downloads and verification can be cancelled; an installer already running owns its outcome.

## Android

Files stay in the application's private no-backup updates directory. A different advertised package or signing key
is refused before downloading. Before installation, Porchlight checks the APK's package, versionCode and signers
against the installed app and advertised release. If Android cannot return archive signing information, the
already-checked advertised signer set is used; Android still enforces signing continuity at install time.

If needed, Allow app updates opens Android's per-app unknown-source permission page. Return to Porchlight to continue;
the APK is reverified. Android then asks for installation confirmation. Failure is shown in the updater and private
staging is cleaned up. Package replacement keeps the existing alarm reschedule/foreground-sync restart behavior.

## Linux desktop

Only an `install.sh` installation supports in-app updates. Other copies explain that they must be updated the way
they were installed. Install `minisign` and `python3` on PATH first.

The updater stages the desktop subset in `${XDG_STATE_HOME:-~/.local/state}/porchlight/updates/<version>/source/`
(directories 0700, files 0600). It runs the **currently installed** trusted installer, not the downloaded installer;
that verifies minisign, the subset and the version before switching `current`. The sibling `installer.log` is outside
the verified source subset. On failure the UI names this log without exposing its contents. Successful staging is
removed; if cleanup fails after installation, it can be removed manually.

Restart to finish starts the stable launcher with `--after-update` and closes the old app. The replacement waits up to
ten seconds for the old singleton lock, then falls back to the normal focus-existing-instance behavior.

The banner, About, server card and 426 updater remain DESIGN-PENDING. JVM UI evidence is generated under
`client/shared/build/test-evidence/updates/`, including 320dp at font scale 2.0. Real Android confirmation and the desktop
installer/subset flow are exercised by the [controller's acceptance gate](update-acceptance.md), not the host-only tests.
Visible desktop process replacement remains a separate manual check.
