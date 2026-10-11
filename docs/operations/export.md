# Data export

Exports are private household data, even though credentials/secrets are excluded. Keep outputs under var/ (private
0700 parent); never attach them to public diagnostics. Use an admin PAT private file for porch's local agent socket.

```bash
umask 022
. scripts/env.sh
porch --socket var/RUN/agent.sock --token-file var/admin.token export --member MEMBER_UUID --out var/member.json
porch --socket var/RUN/agent.sock --token-file var/admin.token export --household --out var/household.tar.gz
python3 -I deploy/lib/verify-export.py var/household.tar.gz
```

`--out` creates0600, refuses overwrite and removes partial output on failure. Never ignore porch's exit code.
Without --out it writes raw bytes to stdout; a failure may already have emitted partial bytes, so consumers must
validate completeness independently. Do not pipe binary household output through a JSON parser.

Member export formatv1 includes current visible tables and `appointments_ics`; save that string as an .ics file.
Current visibility does not grant historical revision access. App callers export themselves; admin PATs may select
another live member but still follow that member's ACL. Appointment materializations are standalone VEVENTs;
TZID-local fixed times, floating-without-TZID, all-day dates and later-fold UTC behavior are documented in
[the data-out contract](../specs/data-out.md). Exports do not guarantee a restorable series definition in another app.

Member preparation is serialized per household; at most two in-progress/completed member spools may be retained.
Additional requests receive 429 with `Retry-After: 1`. Retained bytes remain charged to the disk ledger until
completion, body cancellation or a 30-second idle timeout. Member writes leave an additional disk reserve for
administrative preparation; completed member downloads do not hold household export admission.

Household export is a streaming tar.gz formatv2: small `data.json` manifest, `tables/NAME.jsonl`, blob bytes, and
**LAST complete.json** containing table counts + blob inventory. All members are exported but credentials, sessions,
PATs, step-ups, feed bearer tokens, PKI and filesystem secrets are excluded. Admin agent scope suffices; an mTLS app
caller must be super-admin with single-use step-up action `household.export`. A 429 admission rejection does not
consume that authorization: retry with the same token after the quota clears.

Require final complete.json, matching entries/counts/hashes **and** valid gzip CRC/trailer. An archive without the
marker or a valid trailer is incomplete despite HTTP200. The verifier reads bounded blob chunks and does not
extract/apply data. Delete/replace rejected outputs only after inspecting private diagnostics. Exports are portable
data, **not** disaster-recovery bundles; no generic export importer is implemented. Use age-encrypted independent
backups and [restore drill](backup-restore.md) for full recovery.
