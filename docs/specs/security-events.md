# Security history (0070)

Server-side administrative history; no client UI, notification, domain derivation or device SSE delivery.
The owner-approved server-only scope exempts this table from client feature parity and the transactional outbox:
`security_events` is already the durable event record, and publishing authentication history to devices is inappropriate.
Existing `token.created` outbox metadata remains unchanged.

## Data and attribution

`security_events`: `id`, `household_id`, `occurred_at`, `kind`, `actor`, `target_type`, `target_id`, `outcome`.
The database supplies identity, insertion time and `pl_current_actor()` attribution (member/device/token IDs,
agent label, request ID and DB role). App INSERT grants exclude those generated columns. Actor and target IDs
have no identity FKs, so revocation, member removal and expired-token cleanup cannot erase historical attribution.
No passwords, hashes, bearer tokens, reset/enrollment codes, CSRs, request bodies or free-form diagnostics are recorded.

Kinds: `login_failed`, `login_succeeded`, `password_changed`, `password_reset_issued`,
`password_reset_redeemed`, `session_revoked`, `device_enrolled`, `device_revoked`, `certificate_renewed`,
`token_created`, `token_revoked`, `admin_token_recovered`, `role_changed`, `member_removed`.
Outcomes are `succeeded` or `failed`; targets are member, device or token UUIDs. Session revocations target the
session's device (including rotation and password/reset/member-removal revocations).

Semantic auth/renewal/local-recovery handlers record inside their mutation transaction. Shared state-transition
triggers cover enrollment, reset issuance, revocations and member role/removal writes, including bulk member
removal and local credential issuance. Repeated revocation without a state change writes no new event.
Failed login/password-lockout responses use the committed-business-error path, so a later successful login does
not erase failure history. Same-key replays do not repeat events; transient rolled-back transactions leave none.
Transport/pre-auth failures without a validated household/device, and rate-limit rejections before a mutation
transaction, do not enter this log. This is not a network access log or a record of every rejected HTTP request.

## Reading and retention

`GET /v1/admin/security-events?since=RFC3339&kind=KIND&limit=50&cursor=…` requires an admin PAT on the agent
socket. No mTLS route and no raw agent SQL grant. Returns `{items:[…], next_cursor:…}` in ascending
`(occurred_at,id)` order; since is inclusive. Limit is 1–200; the signed keyset cursor is bound to household and
filters. `porch security-events [--since RFC3339] [--kind KIND] [--json]` follows pages automatically.

Append-only, including TRUNCATE; app cannot update/delete, and raw agent cannot read/insert. Only the existing
maintenance-owned `pl_retention(p_now)` may DELETE, strictly before `p_now - interval '1 year'` (boundary retained).
The nightly job records per-household deleted counts in `purge_log`. No domain or stream derivation is triggered.

Admin household export includes `tables/security_events.jsonl` and its completion inventory count. Member exports
exclude it. The normal whole-database `pg_dump` and catalog-based backup/restore count inventory include it;
`porchlight_backup` inherits `pg_read_all_data`. Retention on the running server does not rewrite old encrypted backups.

## Certificate issuance bound

Under the existing live-device lock, renewal permits at most three certificate issuances per device in a rolling
24 hours, **including enrollment**. Any newer certificate also prevents fresh renewal from an older presented leaf.
These durable checks precede signing/insertion and use indexed certificate history with insertion-time timestamps.
Rejections return 429 `rate-limited`; do not create fresh keys to retry an old leaf. Old leaves remain valid for
ordinary requests until expiry/revocation. The original idempotency key/body can replay its sealed renewal reply,
even after a successor exists or the issuance budget is exhausted (subject to the ordinary 24-hour replay lifetime).

## Dev-init reply recovery

After pinned TLS, before dispatch, `porch dev-init` fsyncs an immutable pending attempt to a 0600 `pending.json`
inside the device's 0700 directory under the selected runtime output directory. It contains the key, CSR,
enrollment reference/pin, original user/label/platform, idempotency key and request digest; **not the password**.
The device directory is exclusively locked while an invocation uses it. Retry re-enters the original password,
validates the request identity and reuses the original enrollment token, key and idempotency key. A lost response
(before headers or during its body) retains this attempt; no replacement device or token is silently created.
A definite rejection on the first attempt can discard an unissued attempt; a resumed attempt is never discarded
on rejection. Successful credential publication removes pending state and syncs the directory. Existing completed
credentials are never overwritten. If the server's replay lifetime has expired, inspect/revoke the affected device
before deliberately replacing its pending directory; keep all such state under `var/`, never in snapshots or logs.
