# Household root CA rollover

Root rollover is **not** server-leaf renewal. Current M2 PKI has one household root pinned to install_state; overwriting
ca.key/ca.crt, restoring unrelated PKI or editing install_state in place breaks every device. Never do that against a
running household. Leaf renewal at startup (<60 days remaining) retains the same root and does not require rollover.

## Planned dual-trust procedure (owner-controlled maintenance)

1. Inventory all enrolled devices/clients and their last-seen state, including the recovery desktop. Take an encrypted
   backup and PASS the isolated restore drill; preserve the matching signed release and old CA privately.
2. Stage a new root and record its DER SHA-256 fingerprint out of band. Generate it with the same private-file custody
   as init, not an ad hoc key copied into live data. Choose a finite dual-trust deadline and maintenance rollback point.
3. **Prerequisite gate:** server/client trust-root distribution and dual-root verification must exist and be tested.
   Push a root/endpoint-list update authenticated by the old mTLS household to every client. The update carries the
   new root/fingerprint, endpoints, activation/deadline and unchanged household identity. Require owner confirmation
   against the out-of-band fingerprint; never learn a root from an untrusted TLS endpoint.
4. During the dual-trust window clients accept old + new household roots; server accepts only unrevoked enrolled
   devices issued by either household root. Switch server leaf/chain to the new root only after each online client
   acknowledges both trust roots. Keep LAN-only enrollment and peer-CIDR checks. Verify resumption remains disabled.
5. Re-enroll/renew each device onto a new-root certificate (same account/permissions, key custody per platform), then
   revoke its old certificate and confirm open streams close. Reconnect, login, sync and test Android background/desktop
   notifications on each device. Offline devices must return to the LAN and enroll before the deadline.
6. At the deadline remove old-root trust from server and clients, revoke remaining old certificates and require
   re-enrollment for missing devices. Run TLS rejection/CA-pinning tests and another backup/restore drill. Keep old
   recovery bundles/CA in protected cold storage according to retention; never leave the old root as permanent trust.

## Current-release stop condition

O1 supplies this runbook, not a CA-rollover API or dual-root client implementation. The current init/serve path pins a
single root; do **not** claim endpoint-list push alone rotates trust roots. If prerequisite step 3 is unavailable, stop
and schedule an owner-approved implementation/maintenance plan. Do not improvise a trust bypass or disable mTLS.
A controlled outage with full device re-enrollment needs a reviewed logical-data/household-identity migration, not a
fresh init over the existing DB. Rollback is the pre-rollover bundle plus the matching old release, and devices already
switched may need explicit old-root re-enrollment. This limitation is a human/review gate, not an executable command.
