#!/usr/bin/env bash
# Creates var/dev.env (mode 0600) with random dev passwords if it does not exist; prints its path.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
file="$root/var/dev.env"
if [ ! -e "$file" ]; then
  mkdir -p "$root/var"
  umask 077
  {
    echo "PG_PORT=5432"
    for k in POSTGRES_PASSWORD PORCHLIGHT_MIGRATOR_PASSWORD PORCHLIGHT_APP_PASSWORD PORCHLIGHT_AGENT_PASSWORD; do
      echo "$k=$(openssl rand -hex 16)"
    done
  } > "$file"
  chmod 600 "$file"
fi
# M2 adds an admin password to existing M1 environments, without rotating DB credentials.
if ! grep -q '^PORCHLIGHT_DEV_ADMIN_PASSWORD=' "$file"; then
  umask 077
  printf 'PORCHLIGHT_DEV_ADMIN_PASSWORD=%s\n' "$(openssl rand -hex 24)" >> "$file"
fi
# O1 adds the read-only backup job login; never rotate existing credentials.
if ! grep -q '^PORCHLIGHT_BACKUP_PASSWORD=' "$file"; then
  umask 077
  printf 'PORCHLIGHT_BACKUP_PASSWORD=%s\n' "$(openssl rand -hex 24)" >> "$file"
fi
# m1's Compose env file must work even without an exported version.
if ! grep -q '^PORCHLIGHT_VERSION=' "$file"; then
  printf 'PORCHLIGHT_VERSION=%s\n' "$(tr -d '\r\n' < "$root/VERSION")" >> "$file"
fi
chmod 600 "$file"
echo "$file"
