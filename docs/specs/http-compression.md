# JSON response compression and snapshot memory

The final HTTP layer negotiates gzip for `application/json` and `application/problem+json` responses of at least
8192 bytes. Explicit `gzip;q=0` overrides a wildcard; absent/invalid permission leaves identity encoding. Eligible
responses carry `Vary: Accept-Encoding` (preserving existing Vary fields), even when small or sent uncompressed.
Gzip uses flate2 level 1. Buffered ordinary responses compress on the blocking executor;
snapshot and item-sync document writers compress incrementally into the encoded buffer with at most a
32 KiB input chunk (plus one row/typed metadata section), without retaining an identity document.
Content-Length describes encoded bytes; strong ETags become weak on encoded representations.
When identity is forbidden, eligible small responses also use gzip; no acceptable representation returns
an empty 406, including secret-excluded routes (which never compress). HEAD carries the selected GET's
Content-Encoding/Vary and omits Content-Length when the encoded length is unknown, without draining a body.
204/304 responses are never compressed or drained by this layer.
Range requests, Content-Range responses and already content-coded responses are untouched.

Compression wraps the complete router, outside the mutation/idempotency layer. The stored response remains the
identity JSON document: replay negotiates afresh, so the original and replay may choose different encodings without
changing status or parsed JSON. The body-transforming layer upgrades an admission body's weak guard before draining
it, then transfers that guard to the encoded body and its retained data frames. Unread compressed snapshots retain
the device/member/global permit; EOF, cancellation and disconnect release it as before.

## Secret separation / BREACH

TLS compression is not enabled. HTTP compression can nevertheless reveal a secret through encoded length when
an attacker can arrange chosen reflected input beside it and observe many responses. Authentication does not by
itself prevent this oracle. These complete response classes (including errors) are excluded:

- `/v1/auth/*`, `/v1/enrollment/*`: session and step-up tokens; enrollment also includes certificates and caller input.
- `/v1/admin/enrollment-tokens`, `/v1/admin/tokens` and children, and user `password-reset` operations: enrollment
  tokens/URIs, PATs and password-reset codes can accompany labels/usernames controlled by another principal.
- `/v1/feeds` and `/v1/admin/feeds`, including children: complete ICS URLs may contain private path/query/user-info
  credentials beside editable feed names. Never compress these, including update/version-conflict responses.
- Attachments/thumbnails, member/household exports, backup downloads and release files: byte/range/digest semantics
  are preserved; exports/backups may additionally contain secrets. SSE is excluded by its content type.

Other compressed household JSON is authorized domain data, not authentication material. Snapshot ICS source
metadata contains only host/name/status, never full URLs, URL user-info, private paths or query tokens. Items,
comments, lists, planning, notification details and their conflict responses do not add server-held bearer secrets.
Arbitrary user-entered text is not treated as a protocol credential. New response classes that place an actual
credential beside reflected input must be excluded before adding them to this general JSON policy.

Both Android and desktop use `OkHttpTrustFactory` and `KtorApiTransport`. Their `HeaderPolicy`, JSON requests and
engine configuration do not set Accept-Encoding or disable OkHttp's bridge interceptor; OkHttp therefore adds gzip
and transparently decompresses JSON responses. Range downloads disable transparent gzip by OkHttp's own Range
rule, and the server never compresses downloads. No client change is required.

## Snapshot construction and measured reference workload

All reads remain in one read-only REPEATABLE READ transaction. Identity documents append sections to one owned
byte buffer, with bounded (one-eighth) spare-capacity growth rather than Vec's default doubling. Negotiated gzip
documents feed that same incremental writer into the encoder; only compressed output and a bounded input chunk
are retained. Timing refinement retains only each item's timing fields, not a second copy of its notes. PostgreSQL returns
JSON text; the writer removes insignificant whitespace without a Value tree. Existing capability decoration is
row-local, and occurrence timing refinement deserializes one typed row at a time. Existing typed metadata
contributors are serialized and released section by section. Commit completes before the body is returned; no
transaction or database cursor is held while the client drains. Admission remains device 1 / member 3 / global 4.

`scripts/check-snapshot-alloc.sh` compares the real old implementation, retained only in the test module, with the
new handler. The fixture has 1000 items, 10000 occurrences, private/selected/household ACLs, a deleted item, detached
occurrences, comments, attachment metadata, a list/entry, reminders, notifications, ICS metadata and planning rows.
Parsed equality also runs for owner, selected viewer and dependent. The isolated counting allocator reports peak
requested-live bytes above the warmed process baseline, including the response buffer; setup is excluded. As with
the ingest probe, allocator-internal overhead/fragmentation and transient System realloc copies are not counted.

Example debug-build measurement on the same fixture (timings are observations, not flaky CI assertions):

| Metric | Old tree writer | New document writer |
| --- | ---: | ---: |
| JSON bytes | 9,283,408 | 9,283,408 |
| Level-1 gzip bytes | 447,763 | 456,628 |
| Peak requested-live heap bytes | 91,699,418 | 10,547,301 |
| Single snapshot, ms | 1047 | 627 |
| Four concurrent snapshots, ms (whole batch) | 2876 | 1588 |
| Level-1 gzip CPU/wall interval, ms | 84 | 71 |

The old production wire representation was uncompressed; its gzip figure above is a comparison measurement.
Section/key ordering differs but parsed JSON is unchanged. Four-concurrent uses three owner devices and one selected
viewer device, respecting the unchanged member admission cap. Compression time is measured separately; snapshot
latencies above are the identity handler. The test asserts peak <=2.5x JSON bytes and the 12 MiB rounded reference
estimate used by AD-03's capacity test. Neither the estimate nor admission truncates unbounded household history;
a larger history still requires a larger per-request working set.

### Shipping-router allocation gate

The same script also exercises the real authenticated router with `Accept-Encoding: gzip`, holding response
bodies/frames through measurement. A 1000-item/10000-occurrence fixture with distinct digest-text notes produces
20,203,408 identity bytes and about 6,685,553 gzip bytes (less compressible than repeated fixture notes). Before
incremental compression, the router peaked at 32,423,649 requested-live bytes. Afterward: 8,162,081 bytes for one
response; four admitted responses retained 24,468,635 encoded bytes and peaked at 29,115,016 bytes. The asserted
bound is 1.5x retained encoded bytes plus 6 MiB per request, including router/encoder/metadata work.

The bundle gate uses a 720-day appointment, 20,000 three-byte note characters, eight requested windows (one
duplicate), hard conflicts, and 401 large comments. Four held identity responses peaked at 34,299,672 bytes
(asserted <=44 MiB); each first page was 8,340,986 bytes (hard bound 8 MiB). All seven deduplicated agenda scopes
and comments were exhausted in 39 pages, whose row digests exactly matched the unbounded shared readers.
Each response asserts at most 200 agenda rows total and 200 comments. These are requested-live allocator
measurements, not RSS/allocator-fragmentation guarantees; timing remains observational.
