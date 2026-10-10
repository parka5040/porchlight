#!/bin/sh
# First start and restore. Secrets cross the process boundary via environment, never argv.
set -eu
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
