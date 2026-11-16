#!/bin/sh
# Read owner-only bind files before the official entrypoint drops to the postgres UID.
# Docker's configured environment contains paths only; values never enter argv.
set -eu
for role in MIGRATOR APP AGENT BACKUP; do
  key=PORCHLIGHT_${role}_PASSWORD
  file_key=${key}_FILE
  eval 'path=${'"$file_key"'-}'
  if [ -n "$path" ]; then
    value=$(cat -- "$path")
    [ -n "$value" ] || exit 1
    export "$key=$value"
    unset "$file_key"
  fi
done
unset value path
exec /usr/local/bin/docker-entrypoint.sh "$@"
