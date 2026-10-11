# Server integration tests

Integration targets use `tests/<area>/main.rs`, with the old filenames retained as modules. Add tests to the
appropriate module/area rather than creating another top-level test binary: every binary links the crate graph.
Shared `tests/common/`, `tests/support/`, fixtures and CLI goldens remain single-sourced outside the area directories.

| Crate | Targets |
| --- | --- |
| api | `trust`, `items`, `calendar`, `collaboration` (also lists), `planning`, `live`, `protocol` (also operations), `calendar_cost`, `derivation_cost`, `concurrency` |
| db | `items`, `collaboration` (calendar/lists/planning/notifications/stream), `operations`, `protocol` |
| core | `domain` (items/agenda/scheduling/lists), `content` (ICS/ingest), `allocation` |
| cli | `commands` |
| pki | `trust` |

From the repository root, source `. scripts/env.sh`, then use `cd server`. For example:

```bash
CARGO_NET_OFFLINE=true cargo test --locked -p porchlight-api --test calendar calendar_http::
CARGO_NET_OFFLINE=true cargo test --locked -p porchlight-cli --test commands domain::
CARGO_NET_OFFLINE=true cargo test --locked -p porchlight-api --test live -- notifications:: stream::
```

Use `module::function` and `-- --exact` to select one test. Full-only commands in `scripts/ci.sh` include the module
prefix for ignored tests; `scripts/check-ingest-alloc.sh` selects the instrumented allocation target and exact test.

## Process isolation

Most modules can share a parallel test process: databases are per SQLx test, listeners bind ephemeral ports, and
filesystem fixtures use unique temporary directories. Tests read recording/seed environment variables but do not
mutate the process environment. Crate-relative runtime paths and SQLx migration paths are unchanged.

Four targets deliberately remain single-module binaries, without changing test logic or assertions:

- `api/tests/calendar_cost/calendar_m7fix.rs`: global TRACE callsite interest plus scoped SQL counters and ignored
  wall-clock/full-cap measurements. Keep other calendar workloads out of its process.
- `api/tests/derivation_cost/fix_b.rs`: installs a global subscriber that counts only its instrumented derivation spans.
- `api/tests/concurrency/mutation_concurrency.rs`: process-global WARN sink/Once keeps tracing interest enabled across
  parallel scoped captures; it must not compete with the statement-count subscribers.
- `core/tests/allocation/ingest_alloc.rs`: the full-only probe supplies a process-global counting allocator. Unrelated
  tests must not contribute allocations. Its helper stays in `tests/support/`.

SQLx receives the target/module/function path, so keep these identities unique even across crates; do not derive
Postgres identifiers by truncating test names. The SQLx harness manages database identifiers and cleanup.

## Checking inventory during another regrouping

Before and after moving modules, save these outputs (replace `before` with `after` on the second pass):

```bash
CARGO_NET_OFFLINE=true cargo test --workspace --locked --no-run --message-format=json > ../var/before-artifacts.jsonl
CARGO_NET_OFFLINE=true cargo test --workspace --locked -- --list --format terse > ../var/before-list.txt
CARGO_NET_OFFLINE=true cargo test --workspace --locked -- --ignored --list > ../var/before-ignored-list.txt
python3 -I ../scripts/check-server-test-inventory.py capture ../var/before-artifacts.jsonl \
  ../var/before-list.txt ../var/before-ignored-list.txt ../var/before.txt
```

Capture while those executables still exist. The script checks the Cargo listing against every test executable,
includes unit tests and doctests, and qualifies old standalone tests with their filename. Grouped tests already
have that module prefix. It rejects duplicate normalized identities and writes a separate ignored inventory.

```bash
python3 -I ../scripts/check-server-test-inventory.py compare ../var/before.txt ../var/after.txt
```

Measure disk usage only after `cargo clean --locked` and a fresh `cargo test --workspace --locked --no-run` on each
layout, using the debug directory under Cargo's configured target directory (not necessarily `server/target`).
