#!/usr/bin/env bash
# M1 acceptance demo: exercises the database guarantees as an agent and checks the outbox projector.
# Requires the dev stack (`scripts/dev-db.sh stack`). Re-runnable: each run creates a fresh household.
# Prints one PASS/FAIL line per check; exits non-zero if any check failed.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
"$root/scripts/dev-env.sh" >/dev/null
. "$root/scripts/env.sh"

admin_url="$PORCHLIGHT_SUPERUSER_URL"
fails=0
run_id=$(date -u +%Y%m%d%H%M%S)-$RANDOM

# Expected failures are captured, not fatal: psql output (stdout+stderr, SQLSTATE via VERBOSITY=verbose)
# is returned and `|| true` keeps `set -e` from aborting. $1 = application_name, rest = psql flags + SQL.
psql_as() { # psql_as <appname> <extra psql flags...> -- <sql>
  local app=$1; shift
  PGAPPNAME="$app" psql -X -At -v VERBOSITY=verbose -v ON_ERROR_STOP=1 "$PORCHLIGHT_AGENT_URL" "$@" 2>&1 || true
}
agent_q() { psql_as "agent:demo" -q -c "$1"; }   # quiet: result rows / errors only
agent_t() { psql_as "agent:demo" -c "$1"; }      # with command tags (UPDATE n, DELETE n)
admin() { PGAPPNAME= psql -X -q -At -v ON_ERROR_STOP=1 "$admin_url" -c "$1" | head -n1; }

check() { # check <n> <description> <actual> <expected-substring>
  if [[ $3 == *"$4"* ]]; then
    echo "PASS: $1 $2"
  else
    echo "FAIL: $1 $2 (got: ${3//$'\n'/ | })"
    fails=$((fails + 1))
  fi
}

# ---- setup (superuser): fresh household with a super admin ----
hid=$(admin "insert into porchlight.households (name, tzid) values ('m1-demo-$run_id', 'UTC') returning id")
uid=$(admin "insert into porchlight.users (household_id, username) values ('$hid', 'demo-$run_id') returning id")
mid=$(admin "insert into porchlight.household_members (household_id, user_id, role, display_name, color, tzid)
             values ('$hid', '$uid', 'super_admin', 'Demo Admin', '#336699', 'UTC') returning id")
echo "setup: household $hid, admin member $mid"

# 1. insert an appointment as the agent
start_ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)   # scopes the api-log check to this run
out=$(agent_q "insert into porchlight.items (household_id, title, owner_id, kind, time_mode, tzid, starts_at, ends_at)
               values ('$hid', 'Dentist', '$mid', 'appointment', 'fixed', 'UTC', now() + interval '1 day', now() + interval '1 day 1 hour')
               returning id")
iid=$out
if [[ $iid =~ ^[0-9a-f-]{36}$ ]]; then
  echo "PASS: 1 agent inserts an appointment item"
else
  echo "FAIL: 1 agent inserts an appointment item (got: ${out//$'\n'/ | })"
  fails=$((fails + 1))
  iid=00000000-0000-0000-0000-000000000000
fi

# 2. blind update bumps the version to 2
agent_q "update porchlight.items set title = 'Dentist (moved)' where id = '$iid'" >/dev/null
out=$(agent_q "select version from porchlight.items where id = '$iid'")
[[ $out == 2 ]] && echo "PASS: 2 blind update makes version 2" ||
  { echo "FAIL: 2 blind update makes version 2 (got: ${out//$'\n'/ | })"; fails=$((fails + 1)); }

# 3. stale optimistic update matches no rows
out=$(agent_t "update porchlight.items set title = 'x' where id = '$iid' and version = 1")
check 3 "update ... and version = 1 is UPDATE 0" "$out" "UPDATE 0"

# 4. explicit write to version is rejected
out=$(agent_q "update porchlight.items set version = 9 where id = '$iid'")
check 4 "explicit version write fails with PL409" "$out" "PL409"

# 5. agents cannot delete
out=$(agent_q "delete from porchlight.items where id = '$iid'")
check 5 "delete is permission denied (42501)" "$out" "42501"
check 5 "delete says permission denied" "$out" "permission denied"

# 6. agent write without agent:<name> application name
out=$(psql_as "" -q -c "update porchlight.items set title = 'y' where id = '$iid'")
check 6 "unlabelled agent write fails with PL403" "$out" "PL403"

# 7. credentials are off limits
out=$(agent_q "select * from porchlight.user_credentials")
check 7 "user_credentials is permission denied (42501)" "$out" "42501"
check 7 "user_credentials says permission denied" "$out" "permission denied"

# 8. revision actor records the agent label
out=$(agent_q "select actor->>'application_name' from porchlight.item_revisions where item_id = '$iid' order by rev desc limit 1")
check 8 "latest revision actor is agent:demo" "$out" "agent:demo"

# 9. the raw agent write enqueued derivation and the M3 derive worker converges it (eventually consistent raw SQL)
out=""
for _ in $(seq 1 30); do
  out=$(agent_q "select requested_gen > 0 and applied_gen = requested_gen from porchlight.item_derivation where item_id = '$iid'")
  [[ $out == t ]] && break
  sleep 0.5
done
[[ $out == t ]] && echo "PASS: 9 derivation enqueued and converged by the worker (applied_gen = requested_gen)" ||
  { echo "FAIL: 9 derivation enqueued and converged by the worker within 15 s (got: ${out//$'\n'/ | })"; fails=$((fails + 1)); }

# 10. projector assigns stream_seq within 10 s, and the api logs it
pending=unknown
for _ in $(seq 1 20); do
  pending=$(admin "select count(*) filter (where stream_seq is null) || '/' || count(*) from porchlight.outbox where entity_id = '$iid'" 2>&1 || true)
  [[ $pending == 0/* && $pending != 0/0 ]] && break
  sleep 0.5
done
if [[ $pending == 0/* && $pending != 0/0 ]]; then
  echo "PASS: 10 outbox rows have stream_seq ($pending unsequenced/total)"
else
  echo "FAIL: 10 outbox rows have stream_seq (got unsequenced/total: $pending)"
  fails=$((fails + 1))
fi
max_seq=$(admin "select coalesce(max(stream_seq), -1) from porchlight.outbox where entity_id = '$iid'" 2>&1 || true)
log_seq=none
for _ in $(seq 1 20); do
  # newest stream_seq logged by the api since this run started (ANSI colours stripped)
  log_seq=$(docker compose -f "$root/deploy/compose.dev.yml" --env-file "$root/var/dev.env" logs --since "$start_ts" api 2>&1 |
    sed -r 's/\x1b\[[0-9;]*[A-Za-z]//g' | sed -n 's/.*stream_seq=\([0-9][0-9]*\).*/\1/p' | sort -n | tail -n1 || true)
  [[ $max_seq =~ ^[0-9]+$ && $log_seq =~ ^[0-9]+$ ]] && ((log_seq >= max_seq)) && break
  sleep 0.5
done
if [[ $max_seq =~ ^[0-9]+$ && $log_seq =~ ^[0-9]+$ ]] && ((log_seq >= max_seq)); then
  echo "PASS: 10 api log since $start_ts shows stream_seq=$log_seq (>= this run's max $max_seq)"
else
  echo "FAIL: 10 api log shows stream_seq >= this run's max (got log: ${log_seq:-none}, max: $max_seq)"
  fails=$((fails + 1))
fi

if ((fails)); then
  echo "m1-demo: $fails check(s) FAILED" >&2
  exit 1
fi
echo "m1-demo: all checks passed"
