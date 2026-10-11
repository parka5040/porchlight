# Problem catalog

Generated from `server/api/src/error.rs`; RFC 9457 bodies use `urn:porchlight:problem:<slug>`.

| Slug | Status | Title |
| --- | --- | --- |
| `calendar-too-large` | 422 | Calendar exceeds apply work budget |
| `invalid-request` | 400 | Invalid request |
| `idempotency-key-required` | 400 | Idempotency key required |
| `csr-invalid` | 400 | Invalid certificate request |
| `unauthenticated` | 401 | Authentication required |
| `device-unknown` | 401 | Unknown device |
| `device-certificate-expired` | 401 | Device certificate expired |
| `request-timeout` | 408 | Request body timed out |
| `device-revoked` | 401 | Device revoked |
| `session-invalid` | 401 | Invalid session |
| `session-device-mismatch` | 401 | Session belongs to another device |
| `invalid-credentials` | 401 | Invalid credentials |
| `reset-code-invalid` | 401 | Invalid reset code |
| `enrollment-token-invalid` | 401 | Invalid enrollment token |
| `forbidden` | 403 | Forbidden |
| `step-up-required` | 403 | Step-up required |
| `not-found` | 404 | Not found |
| `client-release-unavailable` | 404 | Client release unavailable |
| `method-not-allowed` | 405 | Method not allowed |
| `idempotency-in-progress` | 409 | Request in progress |
| `renewal-too-early` | 409 | Renewal too early |
| `enrollment-token-used` | 410 | Enrollment token used |
| `enrollment-token-expired` | 410 | Enrollment token expired |
| `idempotency-key-reused` | 422 | Idempotency key reused |
| `password-policy` | 422 | Password policy |
| `rate-limited` | 429 | Rate limited |
| `account-locked` | 429 | Account locked |
| `purge-dependency-unhandled` | 409 | Purge requires a retention migration |
| `internal` | 500 | Internal server error |
| `maintenance` | 503 | Server is updating |
| `comment-edit-expired` | 403 | Comment edit window expired |
| `attachment-too-large` | 413 | Attachment exceeds 25 MiB limit |
| `storage-full` | 507 | Attachment storage is full |
| `attachment-type-unsupported` | 415 | Unsupported attachment type or WebP main meta-Huffman profile |
| `attachment-type-mismatch` | 422 | Attachment type mismatch |
| `attachment-invalid` | 422 | Invalid attachment |
| `client-version-required` | 400 | Client version required |
| `upgrade-required` | 426 | Upgrade required |
| `invalid-time-zone` | 400 | Invalid time zone |
| `if-match-required` | 428 | If-Match required |
| `version-conflict` | 409 | Version conflict |
| `validation-failed` | 422 | Validation failed |
| `conflict` | 409 | Conflict |
| `range-not-satisfiable` | 416 | Range not satisfiable |

Extensions: `upgrade-required` includes `min_client_version`; `version-conflict` includes `current`; `validation-failed` includes `errors: [{field, message, code?}]`. Body references use `validation-failed` (422), with `invalid-reference` or `stale-neighbour` field codes for board bounds; 404 is reserved for hidden/missing path resources. Idempotency keys cover method, path/query and body, **not If-Match**: rotate the key after `version-conflict`, otherwise the stored response replays. All bodies retain `type`, `title`, `status`, `detail`; internal errors never reveal details.

A sealed 422 (including `stale-neighbour`) replays for its original key. After refetching neighbours or correcting validation, a retry with a changed body needs a **NEW Idempotency-Key**; reusing the old key returns `idempotency-key-reused`.

`maintenance` (503) rejects mutations before authentication, body reads and idempotency while the installer write fence exists. `Retry-After: 1`; retry the identical request after waiting. Reads and healthchecks remain available.
