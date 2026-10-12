#!/usr/bin/env bash
# Dev Postgres: dev-db.sh up | stack | down | psql [postgres|migrator|app|agent] | reset
# Scripts may source the helper; not meant for interactive shells: it sets # Refuse before shell options, root resolution, or filesystem side effects.
if [ -z "${BASH_VERSION:-}" ]; then
  printf '%s: bash is required; refusing without side effects\n' "dev-db" >&2
  # shellcheck disable=SC2317 # return when sourced; exit if return fails in an executed script.
  return 2 2>/dev/null || exit 2
fi
if [[ ${BASH_SOURCE[0]} != "$0" && $- == *i* ]]; then
  printf '%s: interactive sourcing is not supported\n' "dev-db" >&2
  return 2
fi
# shellcheck disable=SC2317 # return when sourced; exit if return fails in an executed script.
script_dir=$(dirname -- "${BASH_SOURCE[0]}") || { return 2 2>/dev/null || exit 2; }
# shellcheck disable=SC2317 # return when sourced; exit if return fails in an executed script.
root=$(cd -- "$script_dir/.." && pwd -P) || { return 2 2>/dev/null || exit 2; }
if [[ $root == / || ! -f $root/scripts/dev-db.sh ]]; then
  printf '%s: invalid project root; refusing\n' "dev-db" >&2
  # shellcheck disable=SC2317 # return when sourced; exit if return fails in an executed script.
  return 2 2>/dev/null || exit 2
fi
set -euo pipefail
# A tree copy (scripts/wt.sh) shares the dev compose project name: `up` from there would recreate the main tree's
# Postgres container with the copy's bind mounts. Copies have their own container; use ci.sh --sandbox there.
if [[ $root == */var/wt/* ]]; then
  printf '%s: tree copy %s uses its own Postgres (scripts/wt.sh); run scripts/ci.sh --sandbox there\n' "dev-db" "${root##*/}" >&2
  exit 2
fi
envfile=$("$root/scripts/dev-env.sh")
set -a
. "$envfile"
set +a
PORCHLIGHT_UID=$(id -u)
PORCHLIGHT_GID=$(id -g)
export PORCHLIGHT_UID PORCHLIGHT_GID
# This helper always addresses the dev project. The M2 demo has its own helper/env file.
export PORCHLIGHT_VAR="$root/var"
# Dev bundles must match the source release, not a stale label in dev.env.
PORCHLIGHT_VERSION=$(tr -d '\n\r' < "$root/VERSION")
export PORCHLIGHT_VERSION
# Postgres creates its socket here; the container's entrypoint takes the directory over on first start.
mkdir -p "$root/var/pgsock"
[[ ! -O $root/var/pgsock ]] || chmod 1777 "$root/var/pgsock"
# Mode/ownership of these binds must allow the non-root api to create PKI and its socket.
mkdir -p "$root/var/server" "$root/var/run" "$root/var/backups" "$root/var/backup-work"
chmod 700 "$root/var/server" "$root/var/run" "$root/var/backups" "$root/var/backup-work"
dc() { docker compose -p porchlight-dev -f "$root/deploy/compose.dev.yml" --env-file "$envfile" "$@"; }

# Sourceable helper above; commands below run only when executed, never when sourced.
if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
case "${1:-}" in
  up) dc up -d --wait postgres ;;
  stack) "$root/scripts/dev-stack.sh" up ;;
  down) dc stop ;;
  psql)
    role=${2:-postgres}
    case "$role" in
      postgres) user=postgres; pw=$POSTGRES_PASSWORD ;;
      migrator) user=porchlight_migrator; pw=$PORCHLIGHT_MIGRATOR_PASSWORD ;;
      app) user=porchlight_app; pw=$PORCHLIGHT_APP_PASSWORD ;;
      agent) user=porchlight_agent; pw=$PORCHLIGHT_AGENT_PASSWORD ;;
      *) echo "unknown role: $role (postgres|migrator|app|agent)" >&2; exit 2 ;;
    esac
    PGPASSWORD="$pw" dc exec -e PGPASSWORD postgres psql -h 127.0.0.1 -U "$user" -d porchlight
    ;;
  reset)
    PGPASSWORD="$POSTGRES_PASSWORD" dc exec -T -e PGPASSWORD postgres psql -h 127.0.0.1 -U postgres -d postgres \
      -v ON_ERROR_STOP=1 -c 'DROP DATABASE IF EXISTS porchlight WITH (FORCE)'
    # Passwords cross the process boundary only through named environment variables.
    { printf '\\getenv migrator_password PORCHLIGHT_MIGRATOR_PASSWORD\n\\getenv app_password PORCHLIGHT_APP_PASSWORD\n\\getenv agent_password PORCHLIGHT_AGENT_PASSWORD\n';
      printf '\\getenv backup_password PORCHLIGHT_BACKUP_PASSWORD\n';
      command cat "$root/deploy/postgres/bootstrap-roles.sql";
    } | PGPASSWORD="$POSTGRES_PASSWORD" dc exec -T -e PGPASSWORD \
      -e PORCHLIGHT_MIGRATOR_PASSWORD -e PORCHLIGHT_APP_PASSWORD -e PORCHLIGHT_AGENT_PASSWORD -e PORCHLIGHT_BACKUP_PASSWORD \
      postgres psql -h 127.0.0.1 -U postgres -d postgres -v ON_ERROR_STOP=1 -f -
    ;;
  *) echo "usage: $0 up|stack|down|psql [role]|reset" >&2; exit 2 ;;
esac
fi
