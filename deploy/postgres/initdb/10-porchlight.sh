#!/bin/sh
# First start and restore. Read credential files natively; old stacks may still supply environment values.
set -eu
for role in MIGRATOR APP AGENT BACKUP; do
  key=PORCHLIGHT_${role}_PASSWORD
  file_key=${key}_FILE
  eval 'path=${'"$file_key"'-}'
  if [ -n "$path" ]; then
    value=$(cat -- "$path")
    [ -n "$value" ] || exit 1
    export "$key=$value"
  fi
done
unset value path
: "${PORCHLIGHT_MIGRATOR_PASSWORD:?PORCHLIGHT_MIGRATOR_PASSWORD is required for bootstrap}"
: "${PORCHLIGHT_APP_PASSWORD:?PORCHLIGHT_APP_PASSWORD is required for bootstrap}"
: "${PORCHLIGHT_AGENT_PASSWORD:?PORCHLIGHT_AGENT_PASSWORD is required for bootstrap}"
: "${PORCHLIGHT_BACKUP_PASSWORD:?PORCHLIGHT_BACKUP_PASSWORD is required; run scripts/dev-env.sh before cluster creation}"
# -f fails loudly on a missing/directory bind; no pipe can hide that error.
psql -X --username "${POSTGRES_USER:-postgres}" --dbname postgres -v ON_ERROR_STOP=1 \
  -c '\getenv migrator_password PORCHLIGHT_MIGRATOR_PASSWORD' \
  -c '\getenv app_password PORCHLIGHT_APP_PASSWORD' \
  -c '\getenv agent_password PORCHLIGHT_AGENT_PASSWORD' \
  -c '\getenv backup_password PORCHLIGHT_BACKUP_PASSWORD' \
  -f /porchlight/bootstrap-roles.sql
