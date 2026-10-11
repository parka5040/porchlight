#!/usr/bin/env bash
# Local CI for Porchlight (there is no hosted CI). Every task must leave this green.
#
# Usage: scripts/ci.sh            fast checks (run after every task)
#        scripts/ci.sh --full     also slow checks: packaged artifacts, emulator/managed-device tests
#        scripts/ci.sh --sandbox  server + offline client checks, without Docker or network: expects the dev
#                                 Postgres to be up already (reached over var/pgsock); skips packaging and devices
#
# Each milestone appends its checks here; keep steps ordered fast to slow.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
full=0
sandbox=0
case ${1:-} in
  "") ;;
  --full) full=1 ;;
  --sandbox) sandbox=1 ;;
  *) echo "usage: scripts/ci.sh [--full|--sandbox]" >&2; exit 2 ;;
esac

step() { printf '\n==> %s\n' "$*"; }

step "VERSION matches the Cargo workspace version"
version=$(tr -d '[:space:]' < "$root/VERSION")
cargo_version=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p}' "$root/server/Cargo.toml")
[[ $version == "$cargo_version" ]] || { echo "VERSION=$version but server/Cargo.toml=$cargo_version" >&2; exit 1; }

step "machine-specific values stay in .env, out of committed files"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/scripts/test-check-private-values.py"
python3 -I "$root/scripts/check-private-values.py"

step "Rust source reachability and release/dependency contracts"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/scripts/test-audit-rust.py"
python3 -I "$root/scripts/check-rust-modules.py"

step "dev database up, test environment"
if (( sandbox )); then
  [[ -S $root/var/pgsock/.s.PGSQL.5432 ]] || { echo "dev Postgres socket missing: ask the owner to run scripts/dev-db.sh up" >&2; exit 1; }
else
  "$root/scripts/dev-db.sh" up
fi
. "$root/scripts/env.sh"

cd "$root/server"

step "migrate dev database (as migrator)"
cargo run -q -p porchlight-api --bin porchlight --locked -- migrate

step "cargo fmt"
cargo fmt --all -- --check

step "cargo clippy"
cargo clippy --workspace --all-targets --locked -- -D warnings

step "sqlx offline query cache is current"
SQLX_OFFLINE=false DATABASE_URL="$PORCHLIGHT_SUPERUSER_URL" \
  cargo sqlx prepare --workspace --check -- --all-targets --locked

step "installer timezone preflight matches server storage zones"
cargo run -q --locked -p porchlight-core --example storage-zones -- --check

step "cargo test"
cargo test --workspace --locked

step "layering: porchlight-core and porchlight-pki are framework-free"
for crate in porchlight-core porchlight-pki; do
  framework_deps=$(cargo tree -p "$crate" -e normal --prefix none --locked)
  if grep -Eq '^(axum|hyper|sqlx|tokio)(-[a-z0-9-]+)? v' <<<"$framework_deps"; then
    echo "$crate must not depend on axum, hyper, sqlx or tokio" >&2
    exit 1
  fi
done

step "layering: porchlight-db has no HTTP or TLS frameworks"
framework_deps=$(cargo tree -p porchlight-db -e normal --prefix none --locked)
if grep -Eq '^(axum|hyper|rustls)(-[a-z0-9-]+)? v' <<<"$framework_deps"; then
  printf 'porchlight-db must not depend on axum, hyper or rustls\n' >&2
  exit 1
fi

step "transactions: top-level BEGIN only through porchlight_db::tx (cancel-safe)"
# sqlx 0.9 Pool::begin / Connection::begin dropped mid-BEGIN strand an open transaction on a pooled
# connection (exportflake). Non-test src opens top-level transactions only via porchlight_db::tx::{begin,
# begin_as}; savepoints are `.begin()` on a Transaction named tx / tx.tx / self.tx. A sanctioned raw call
# carries `// tx-guard: allow`. R86: a transaction whose inner work was cancelled (timeout, select!, drop)
# is poisoned (a cancelled savepoint begin silently rolls it back into autocommit): never reuse it; record
# follow-up state in a fresh begin_as transaction that re-takes its locks (see calendar/jobs.rs).
tx_src=(api/src db/src cli/src core/src pki/src)
begin_sites=$(
  {
    grep -rnE --include='*.rs' '\.(try_)?begin(_with)?\(' "${tx_src[@]}" \
      | grep -vE '(^|[^[:alnum:]_.])(self\.)?tx(\.tx)?\.begin\(\)'
    grep -rnE --include='*.rs' '(Pool|Connection|Acquire|Transaction|TransactionManager)::(try_)?begin' "${tx_src[@]}"
    grep -rniE --include='*.rs' '"[[:space:]]*(begin|start[[:space:]]+transaction)([[:space:]]|;|")' "${tx_src[@]}"
  } | grep -vE '^[^:]*(_tests\.rs|/testing\.rs):|tx-guard: allow' || true
)
if [[ -n $begin_sites ]]; then
  printf 'open top-level transactions with porchlight_db::tx::begin/begin_as, not:\n%s\n' "$begin_sites" >&2
  exit 1
fi

step "mutation callbacks use their IdemTx connection"
python3 -I "$root/scripts/check-mutation-context.py" --self-test
python3 -I "$root/scripts/check-mutation-context.py"

# s1: OpenAPI document is current
step "OpenAPI document is current"
cargo run -q -p porchlight-api --bin porchlight --locked -- openapi | diff -u "$root/docs/openapi.json" -

# s2b + s4a + s4b: one generated core/HTTP permissions document and one gate.
step "endpoint permissions matrix is current"
cargo run -q -p porchlight-api --example endpoint-matrix --locked -- --check
python3 "$root/scripts/check-items-reads.py"
python3 -I "$root/deploy/tests/audit-items-reads.py"

# s6a: workspace tests cover routing/notifier/TLS matrix/CLI; keep docs and demo syntax current.
step "notification contracts and server demo syntax"
python3 "$root/scripts/check-notifications.py"
bash -n "$root/deploy/scripts/m5-demo.sh"
python3 -I "$root/deploy/tests/notifications-stream.py"
python3 -I "$root/deploy/tests/audit-bl-tests-notifications-stream.py"

# s6b: stream tests run in the workspace step; exercise the controller's isolated SQL helper too.
step "stream controller helper and gate syntax"
python3 -I "$root/deploy/tests/stream-db.py"
bash -n "$root/deploy/scripts/m5-demo-stream.sh"
python3 - "$root/deploy/scripts/m5-demo-stream.sh" <<'PY'
import ast, pathlib, sys
source = pathlib.Path(sys.argv[1]).read_text().split("<<'PY'\n", 1)[1].rsplit('\nPY\n', 1)[0]
ast.parse(source)
print('Stream controller gate syntax is current')
PY

# s3: additive authoritative schema and background lock protocols stay documented.
step "derivation matrix and lock-order contract are current"
python3 "$root/scripts/check-derivation.py"

# s8a: calendar fixtures, local TLS/SSRF, feed deletion/read-only, HTTP/CLI and schema docs.
step "M7 ICS/feed gate and additive calendar contract"
bash -n "$root/deploy/scripts/m7-demo-ics.sh"
for name in ics_sources ics_uid_map ics_source_revisions ics_source_id expanded_start; do
  grep -q "\`$name\`" "$root/docs/specs/derivation-matrix.md"
done

# s8b: workspace tests cover planning; the real-stack gate is controller-only.
step "planning stack gate syntax"
bash -n "$root/deploy/scripts/m7-demo-planning.sh"

# c6: real handler equality runs in workspace tests; JVM/host suites exercise repositories and panels.
step "C6 planning/import recordings match canonical client contracts"
python3 -I "$root/scripts/check-c6-contract.py"

# c1a + c1b: token contrast, resume, shell accessibility, trust contracts, production local TLS, keystores,
# accessible screens and API-level lint (offline in the sandbox).
# c3a: shared suites include Today/Board/editor projections, immutable move/relock replay and Compose semantics.
# c5: these suites also cover collaboration receipts/fixtures, list neighbour replay, uploads/private downloads and UI.
# ctrust: durable dropped-enrollment replay/restart, R78 outcome classification/Start over, renewal backoff/foreground/transport fencing, lifecycle snapshots.
# Keep Gradle CPU capped to four workers (including nested gate commands).
step "client JVM/host tests, Android apps, desktop jar and lint"
# Non-incremental Kotlin: tree copies merged into main left stale incremental caches ("FqNames can't be derived from
# DirtyData") that failed CI; a clean module compile costs little next to the test run.
gradle_flags=(-Pkotlin.incremental=false)
(( sandbox )) && gradle_flags+=(--offline)
(cd "$root/client" && ./gradlew --max-workers=4 "${gradle_flags[@]}" --console=plain -q :shared:jvmTest :shared:testAndroidHostTest \
  :androidApp:assembleDebug :androidApp:assembleDebugAndroidTest :desktopApp:jar :androidApp:lintDebug :shared:lintAndroidMain \
  :androidApp:assembleRelease) # unsigned: release-only classpath/R8/task-wiring gaps (signed APKs: scripts/release.sh)

step "shrunk Android release artifacts and runtime dependency hygiene"
python3 -I "$root/deploy/tests/audit-apk.py"
# Compile the isolated release-like smoke here; only the controller runs it on managed devices.
(cd "$root/client" && ./gradlew --max-workers=4 "${gradle_flags[@]}" --console=plain -q -Pporchlight.minifiedSmoke=true \
  :androidApp:assembleMinified :androidApp:assembleMinifiedAndroidTest :androidApp:lintMinified)
python3 -I "$root/deploy/tests/audit-apk-fix1.py"
python3 -I "$root/deploy/tests/audit-managed-results.py"

# c5: offline recordings and private binary-cache/picker boundary, in addition to common/JVM/host suites above.
step "C5 collaboration client contract boundaries"
python3 -I "$root/scripts/check-c5-client.py"

# c2a: generated M2 contract compiles/decodes fixtures and GMD excludes controller-only tests.
step "generated client contract and managed-device filter"
(cd "$root/client" && ./gradlew --max-workers=4 "${gradle_flags[@]}" --console=plain -q :contract-check:jvmTest :androidApp:verifyManagedDeviceFilter)
step "controller-only filter expected failure and release fixture boundary"
mkdir -p "$root/var"
if (cd "$root/client" && ./gradlew --max-workers=4 -Pkotlin.incremental=false --offline --console=plain :androidApp:verifyManagedDeviceFilter \
    -Pandroid.testInstrumentationRunnerArguments.notAnnotation=org.junit.Ignore) > "$root/var/c2a-filter-negative.log" 2>&1; then
  printf 'controller-only filter negative probe unexpectedly passed\n' >&2; exit 1
fi
grep -q 'Managed-device runs must exclude @ControllerOnly' "$root/var/c2a-filter-negative.log"
"$root/scripts/check-c2a-wiring.sh"

# c2b: the real item/agenda/board/snapshot recording equality test (api/tests/items/client_item_fixtures.rs) runs in
# `cargo test` above; :shared generates directly from docs/openapi.json with C2a's conventions; ItemFixtureTest,
# repository/cache common tests and DesktopCacheTest run in the shared host suites above.

# o1: offline operations checks; real Docker/signing/restore gates remain controller-run.
step "O1 shell syntax and optional shellcheck"
o1_scripts=(scripts/release.sh scripts/publish-release.sh scripts/release-keys.sh scripts/dev-backup-role.sh scripts/dev-env.sh scripts/dev-db.sh
  scripts/dev-stack.sh scripts/dev-lan.sh
  deploy/install.sh deploy/backup/backup.sh deploy/scripts/restore-drill.sh deploy/lib/operations.sh
  deploy/tests/operations.sh deploy/tests/desktop.sh deploy/tests/github-upgrade.sh deploy/tests/backup.sh deploy/tests/release.sh deploy/tests/publish.sh deploy/postgres/initdb/10-porchlight.sh)
for script in "${o1_scripts[@]}" scripts/env.sh deploy/scripts/m1-demo.sh deploy/scripts/m2-demo.sh; do bash -n "$root/$script"; done
if [[ -x /usr/bin/shellcheck ]]; then
  for script in "${o1_scripts[@]}"; do /usr/bin/shellcheck -e SC1090,SC1091 "$root/$script"; done
else
  printf 'SKIP shellcheck: /usr/bin/shellcheck not installed\n'
fi
step "O1 manifest, version, source, retention, schedule, desktop and backup fixture tests"
python3 - "$root" <<'PY'
from pathlib import Path
import sys
for name in ['deploy/lib/operations.py', 'deploy/backup/backup.py', 'scripts/build-installer.py', 'deploy/tests/compose-env.py']:
    compile(Path(sys.argv[1], name).read_text(), name, 'exec')
PY
python3 "$root/deploy/tests/compose-env.py"
step "development LAN discovery (offline fake ip)"
python3 -I "$root/scripts/test-dev-lan.py"
for script in operations desktop github-upgrade backup release publish; do bash "$root/deploy/tests/$script.sh"; done
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/audit-installer.py"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/audit-installer-fix1.py"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/audit-installer-fix2.py"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/audit-final-deploy.py"
# The real quiesce/resume SQL on a private throwaway cluster (mocked Docker cannot see predicate order).
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/agent-quiesce.py"
step "backlog deploy phases, viewer handoff and Funnel controller contracts"
for test in audit-bl-deploy audit-bl-deploy-funnel audit-bl-deploy-controller; do
  PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/$test.py"
done
step "audit operations resource, recovery and build footprint contracts"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/audit-ops.py"
bash -n "$root/deploy/backup/client-tools.sh" "$root/deploy/scripts/update-gate-build.sh" \
  "$root/scripts/c1b-desktop-e2e.sh" "$root/scripts/c1b-emulator-e2e.sh" "$root/deploy/scripts/m3-demo.sh"

# M9.3 LAN deployment controller: offline shims in both host and sandbox modes.
step "M9 deploy-server shell checks and offline transport contracts"
for script in scripts/deploy-server.sh deploy/tests/deploy-server.sh; do
  bash -n "$root/$script"
  if [[ -x /usr/bin/shellcheck ]]; then /usr/bin/shellcheck "$root/$script"; fi
done
bash "$root/deploy/tests/deploy-server.sh"

# m9-launcher: pre-JVM Wayland decision and transparent exec, offline in both CI modes.
step "M9 desktop launcher wrapper contract"
sh -n "$root/client/desktopApp/launcher/porchlight.sh"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/desktop-launcher.py"

step "F1-02 Linux-x64 SQLite packaging and bundled desktop JVM smoke"
(cd "$root/client" && ./gradlew --max-workers=4 "${gradle_flags[@]}" --console=plain -q :shared:packagedDesktopSmoke)
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/audit-desktop.py"

# o2a: DB/HTTP/CLI/stream/lease tests run above; cheap server and offline producer contracts.
step "O2 server data-out and backup lease contracts"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/scripts/check-data-out.py"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/o2-backup.py"
# m8fix: hostile bootstrap cwd and DB-authoritative backup/independent-drill integrity.
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/installer-isolation.py"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/backup-integrity.py"
# B1 guard: installer-side Python always runs isolated (-I), so a planted module in the cwd is never imported.
if grep -nE '(^|[^[:alnum:]_/])python3([[:space:]]|$)' "$root/deploy/install.sh" "$root/deploy/lib/operations.sh" \
    | grep -v 'python3 -I'; then
  echo "installer Python must run with -I (see m8fix B1)" >&2; exit 1
fi
# Serving-process + pinned TLS readiness regressions run in cargo test above.
# Audit trust: quota/cache/recovery regressions run in workspace tests; never mint credentials in a help gate.
step "Trust recovery and token expiry CLI surfaces"
cargo run -q -p porchlight-api --bin porchlight --locked -- admin-token --help > /dev/null
cargo run -q -p porchlight-cli --bin porch --locked -- tokens list --help > /dev/null

# Audit log: real mutation/retention/export/reply-loss regressions run in workspace tests above.
step "Security history, durable renewal budget and enrollment recovery contracts"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/scripts/check-security-events.py"
cargo run -q -p porchlight-cli --bin porch --locked -- security-events --help > /dev/null

# o2b: fake-server pull tests run in shared:jvmTest above; live M8 gate stays controller-only.
step "O2 desktop passive pull, export completeness and M8 controller contracts"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/export-complete.py"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/m8-contract.py"
# Remote-update acceptance is controller-only; CI checks its isolation and private driver contracts.
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/update-gate-contract.py"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/fbc-backup-ops.py"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/pulled-bundle.py"
bash -n "$root/deploy/scripts/restore-drill.sh"

# m9-nix: offline host prerequisites contract, in both sandbox and host modes.
step "M9 NixOS host module contract"
PYTHONDONTWRITEBYTECODE=1 python3 -I "$root/deploy/tests/nixos-module.py"

# m9-funnel: generated-installer fixtures in both modes; real Compose config only on the host.
step "M9 Funnel and server-cert installer contracts"
bash -n "$root/deploy/tests/funnel.sh"
if [[ -x /usr/bin/shellcheck ]]; then /usr/bin/shellcheck "$root/deploy/tests/funnel.sh"; fi
if (( sandbox )); then
  bash "$root/deploy/tests/funnel.sh" --sandbox
else
  bash "$root/deploy/tests/funnel.sh"
fi

# s7b: collaboration HTTP/CLI tests run in cargo test above; keep schema/demo contracts current.
step "S7b collaboration schema and controller demo syntax"
python3 "$root/scripts/check-collaboration.py"
python3 "$root/scripts/generate-filename-categories.py" --check
python3 "$root/scripts/test-filename-categories.py"
bash -n "$root/deploy/scripts/m6-demo-collab.sh"
if [[ -x /usr/bin/shellcheck ]]; then
  /usr/bin/shellcheck "$root/deploy/scripts/m6-demo-collab.sh"
fi

# s7a: list DB/HTTP/matrix/CLI tests run in cargo test above; deployed gate stays controller-only.
step "S7a lists acceptance script syntax and CLI registration"
bash -n "$root/deploy/scripts/m6-demo-lists.sh"
python3 "$root/scripts/check-lists-demo.py" "$root/deploy/scripts/m6-demo-lists.sh"
cargo run -q -p porchlight-cli --bin porch --locked -- lists --help > /dev/null

# s5: gate syntax is safe offline; live Compose remains under --full.
step "S5 M3 gate shell and Python syntax"
bash -n "$root/deploy/scripts/m3-demo.sh"
python3 - "$root/deploy/scripts/m3-demo.py" <<'PY'
from pathlib import Path
import sys
compile(Path(sys.argv[1]).read_text(), sys.argv[1], 'exec')
PY

# c3b: emulator process safety is deterministic; device execution remains controller-only.
step "C3b M4 gate script syntax and stale netsimd process seam"
bash -n "$root/scripts/emulator-common.sh" "$root/scripts/test-managed-clients.sh" "$root/scripts/smoke-clients.sh" "$root/deploy/scripts/m4-gate.sh"
bash "$root/scripts/test-emulator-cleanup.sh"

# c4: live-client gate tooling compiles offline; actual 30-minute device/session-bus gate is controller-only.
step "C4 M5 client host driver assertions and fresh-AVD soak syntax"
PYTHONDONTWRITEBYTECODE=1 python3 -B -I "$root/deploy/tests/m5-client-gate-test.py"
bash -n "$root/deploy/scripts/m5-demo.sh" "$root/deploy/scripts/m5-client-demo.sh" "$root/deploy/scripts/m5-soak.sh"
python3 -I - "$root/deploy/scripts/m5-client-gate.py" <<'PY'
from pathlib import Path
import sys
compile(Path(sys.argv[1]).read_text(), sys.argv[1], 'exec')
PY

if (( sandbox )); then
  printf '\nci (sandbox): OK\n'
  exit 0
fi

if (( full )); then
  step "M3a opt-in randomized carry model (prints replay seed)"
  PORCHLIGHT_MODEL_SEED=random cargo test --locked -p porchlight-core --test domain items_carry_model::r63_contract_over_operation_sequences_second_fixed_seed -- --nocapture
  step "M7 measured full-cap calendar apply under production deadline and large parse fixture"
  cargo test --locked -p porchlight-api --test calendar_cost calendar_m7fix::fix2_full_cap_production_deadline -- --ignored --nocapture
  cargo test --locked -p porchlight-api --test calendar_cost calendar_m7fix::fix1_r12_ten_thousand_expanded_instances -- --ignored
  cargo test --locked -p porchlight-api --test calendar_cost calendar_m7fix::fix1_worked_calendar_refresh_and_clock_drift -- --ignored
  cargo test --locked -p porchlight-api --test calendar_cost calendar_m7fix::fix1_near_limit_calendar_admits_one_change -- --ignored
  cargo test --locked -p porchlight-api --test calendar_cost calendar_m7fix::fix3_full_cap_removal_apply -- --ignored --nocapture
  cargo test --locked -p porchlight-api --test calendar_cost calendar_m7fix::fix3_full_cap_change_apply -- --ignored --nocapture
  # k6: slow release allocation probe stays out of fast workspace tests.
  step "k6: measured attachment-ingest allocation admission (release)"
  "$root/scripts/check-ingest-alloc.sh"

  # c3b: full managed suite API35/36, ControllerOnly excluded; stale netsimd reaped around boot/shutdown.
  step "Android shell, trust, calendar/settings accessibility, recreation and insets suite (API 35 + 36)"
  "$root/scripts/test-managed-clients.sh"

  step "client smoke: launch APK on emulator and packaged desktop app"
  "$root/scripts/smoke-clients.sh"

  step "release image builds, migrate and init precede healthy postgres + api + sidecar"
  "$root/scripts/dev-stack.sh" up
  api_state=$(docker inspect --format '{{.State.Health.Status}}' porchlight-dev-api-1)
  [[ $api_state == healthy ]] || { echo "api is $api_state, expected healthy" >&2; exit 1; }
  migrate_state=$(docker inspect --format '{{.State.Status}} {{.State.ExitCode}}' porchlight-dev-migrate-1)
  [[ $migrate_state == "exited 0" ]] || { echo "migrate is '$migrate_state', expected 'exited 0'" >&2; exit 1; }
  # Captured first: under pipefail, grep -q closing the pipe early could fail docker inspect and hide a match.
  api_env=$(docker inspect --format '{{range .Config.Env}}{{println .}}{{end}}' porchlight-dev-api-1)
  if grep -q '^PORCHLIGHT_MIGRATOR_URL=' <<<"$api_env"; then
    echo "the api container must not receive the migrator credential" >&2
    exit 1
  fi

  # s8a: operator gate owns an isolated Compose project; workspace tests already ran once.
  step "M7 ICS/feed isolated Compose gate"
  "$root/deploy/scripts/m7-demo-ics.sh"

  step "M1 acceptance demo (database guarantees + outbox projector)"
  "$root/deploy/scripts/m1-demo.sh"

  step "M2 acceptance demo (TLS, enrollment, sessions, revocation, PATs and LAN boundary)"
  # CI diagnostics are runtime state; only the operator writes the kept gate/reboot records.
  mkdir -p "$root/var/gates"
  "$root/deploy/scripts/m2-demo.sh" 2>&1 | tee "$root/var/gates/m2-ci.log"

  # s5: M3 end-to-end gate uses only its isolated Compose project.
  step "M3 acceptance demo (items, recurrence, derivation, CLI, revert and purge)"
  "$root/deploy/scripts/m3-demo.sh" 2>&1 | tee "$root/var/gates/m3-ci.log"
fi

printf '\nci: OK\n'
