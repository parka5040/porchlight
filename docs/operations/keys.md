# Key custody (human gate)

`release-keys.sh` is an operator-only one-time command. `var/release/` is 0700; minisign key/public file, Android
`porchlight-release.jks`, `keystore.pass`, and private generation diagnostics are 0600. Any existing directory,
including a failed partial generation, is refused. Do not delete partial keys automatically: inspect privately and
preserve any successfully generated signing identity before deciding how to recover.

Laptop pass uses **passwordless minisign** (`-W`): possession of `minisign.key` permits signing. Protect the laptop,
filesystem permissions and personal encrypted/offline backups accordingly. The script prints only file paths and
SHA-256 of decoded minisign public-key bytes. Record the public fingerprint out of band to bootstrap installer trust.
The Android JKS alias is `porchlight`; store/key password is generated into `keystore.pass`, passed to keytool through
file options and never placed in source, argv or logs. Losing the APK signing key requires phone reinstalls.

## Committed public anchors

`deploy/release-keys/minisign.pub` is the public release trust anchor;
`deploy/release-keys/android-signer.sha256` pins the complete set of APK signer-certificate SHA-256 digests (one
lowercase 64-hex digest per signer, with optional `#` comments and blank lines). These files contain only public
values. `release.sh` refuses a different local minisign public key before building and refuses a different APK signer
after building. `publish-release.sh` re-verifies the signature against the committed anchor and checks the APK signer
against both the pin and the signed release metadata.

To re-derive the Android pin, use a trusted previously released APK and the configured SDK's `apksigner`:

```bash
. scripts/env.sh
"$ANDROID_HOME/build-tools/<build-tools-version>/apksigner" verify --print-certs dist/<V>/porchlight-<V>.apk
```

Copy **every** `Signer #N certificate SHA-256 digest:` value, lowercase and deduplicated/sorted, into the pin file
only after checking it against the intended signing identity. Do not use the file SHA-256 or a public-key digest
instead of the **certificate** digest. A new APK signing key means **every phone reinstalls** and loses its old local
enrollment; deliberately change the committed pin and plan re-enrollment. Never silently accept the new signer just
to make a failed build pass. The minisign fingerprint is SHA-256 of the base64-decoded final line of its public key
file (the same fingerprint `release-keys.sh` prints), not SHA-256 of the text file.

## Admin PAT rotation and local recovery

The bootstrap PAT expires after one year. Keep a working admin PAT in a 0600 file and rotate before expiry with
`porch token create --name <label> --scope admin --days 365` (normal issuance still requires password re-entry).
`porch --socket <agent-socket> --token-file <private-pat-file> --json tokens list` lists metadata including
`expires_at`, `last_used_at`, `revoked_at` and `admin_token_health`. The health fields describe the **most recently
created live admin PAT** belonging to a live super admin: `newest_live_expires_at`, `expiring_soon` (within 30 days),
and `recovery_required` (none remain). The server logs a warning at startup and hourly for approaching expiry or
no remaining live admin PAT. These are administration warnings, not serving-readiness failures.

If every admin PAT is expired, revoked or lost, the stack owner can mint a replacement **on the server host**,
inside the running API container, using its own app-role database credential. No migrator/superuser password,
old PAT, password reset, re-initialization or auth-table edits are needed. This is a privileged host operation,
not an HTTP authentication bypass. Replace all placeholders with the installed stack's settings:

```bash
# Owner only. Private output stays under the stack's var/; never display it or enable shell tracing.
umask 077
mkdir -p <stack-root>/var
set -o noclobber
docker compose --env-file <stack-root>/.env \
  -f <stack-root>/current/deploy/compose.yml -p <compose-project> \
  exec -T api /usr/local/bin/porchlight admin-token \
  --member <member-uuid-or-username> --name recovery --expires-days 365 \
  > <stack-root>/var/recovered-admin.pat
```

`--member` accepts the live super admin's member UUID or login username; the household has no email identity
field. The command verifies the installation and member, commits one admin-scope PAT through the ordinary token
insertion function, attributes `token.created` to `agent:local-recovery` with the **new** token id, and writes the
credential once to stdout (never to logs). Issuance attribution follows normal outbox retention. Protect the host's
app credential: it already controls household writes. If output fails after commit, rerun into a new private file,
then revoke the unused token by id from `tokens list`.

Use the new file with `porch --socket <agent-socket> --token-file <stack-root>/var/recovered-admin.pat whoami`,
then confirm `tokens list` and enrollment work. Revoke lost/obsolete tokens and keep the replacement in owner
custody. Recovery does not revive expired/revoked tokens or change normal listener authentication.

## Backups

Owner must back up together:
- `var/release/minisign.key` and `minisign.pub` (release authenticity);
- `var/release/porchlight-release.jks` and `keystore.pass` (Android upgrade identity);
- laptop `var/backup-recovery.key` (age identity), separately from server/bundle storage;
- published signed release directories (matching-version installers/images for restores).

Back up to the owner's password manager/personal encrypted offline backup, verify readable recovery on the laptop,
and record that human check before shipping. Do not print private material, put it in docs/snapshots or upload it.
Only the age **public recipient** goes to `porchlight init`/`data/backup-recipient.txt`; the private identity stays on
the laptop and the owner's backup. `age-keygen -y var/backup-recovery.key` yields the public recipient without changing
the identity. Recovery-key generation is a human step on the laptop, never an automated server action.

Release signing configuration takes paths through environment or Gradle properties, not plaintext passwords.
Signed Gradle invocations must use `--no-configuration-cache` (enforced before reading the password file); release.sh
does this automatically so Gradle does not serialize the signing password into client/.gradle. CI
must run without production signing settings. Changing the release minisign key requires distributing a newly trusted
installer/key fingerprint out of band; an existing pinned updater must not silently trust a new key from an artifact.
