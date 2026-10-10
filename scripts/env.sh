# Source this file: `. scripts/env.sh`. Sets up the project toolchain for the current shell.
_porchlight_root=$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)
# Machine paths come from the gitignored .env (see .env.example); unset keeps the shell's own values.
. "$_porchlight_root/scripts/local-env.sh"
porchlight_local_env "$_porchlight_root"
unset -f porchlight_local_env
[ -z "${PORCHLIGHT_JAVA_HOME:-}" ] || export JAVA_HOME="$PORCHLIGHT_JAVA_HOME"   # Gradle and jpackage need JDK 25
[ -z "${PORCHLIGHT_ANDROID_HOME:-}" ] || export ANDROID_HOME="$PORCHLIGHT_ANDROID_HOME"
export PATH="$_porchlight_root/scripts/bin:$JAVA_HOME/bin:$ANDROID_HOME/emulator:$ANDROID_HOME/platform-tools:$ANDROID_HOME/cmdline-tools/latest/bin:$HOME/.cargo/bin:$PATH"
# Project-local Gradle cache and Android user home (debug keystore, AGP state), so sandboxed agents without network
# and without ~/.gradle or ~/.android build offline from what the controller cached. Set before var/dev.env so a
# tree copy's dev.env may point them elsewhere. adb and the emulator ignore ANDROID_USER_HOME (AVDs stay in
# ~/.android/avd); Gradle Managed Devices keep their AVDs under it.
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$_porchlight_root/var/gradle-home}"
export ANDROID_USER_HOME="${ANDROID_USER_HOME:-$_porchlight_root/var/android-user-home}"
if [ -f "$_porchlight_root/var/dev.env" ]; then
  set -a
  . "$_porchlight_root/var/dev.env"
  set +a
  # Over the dev Postgres unix socket (var/pgsock, see deploy/compose.dev.yml) so sandboxed tools without network
  # reach it too; TCP stays published on 127.0.0.1:$PG_PORT. sqlx rejects an empty URL host, hence `localhost`
  # (the `host` parameter overrides it).
  _pg="?host=$_porchlight_root/var/pgsock"
  # A workstream tree copy (var/wt/<name>, scripts/wt.sh) sets PORCHLIGHT_DB_NAME in its own var/dev.env.
  _db=${PORCHLIGHT_DB_NAME:-porchlight}
  export DATABASE_URL="postgres://postgres:$POSTGRES_PASSWORD@localhost/postgres$_pg"
  export PORCHLIGHT_SUPERUSER_URL="postgres://postgres:$POSTGRES_PASSWORD@localhost/$_db$_pg"
  export PORCHLIGHT_MIGRATOR_URL="postgres://porchlight_migrator:$PORCHLIGHT_MIGRATOR_PASSWORD@localhost/$_db$_pg"
  export PORCHLIGHT_DATABASE_URL="postgres://porchlight_app:$PORCHLIGHT_APP_PASSWORD@localhost/$_db$_pg"
  export PORCHLIGHT_AGENT_URL="postgres://porchlight_agent:$PORCHLIGHT_AGENT_PASSWORD@localhost/$_db$_pg"
  unset _pg _db
  export SQLX_OFFLINE=true
fi
# Compose interpolates disabled profiles too; every dev caller needs the repository version.
export PORCHLIGHT_VERSION="$(tr -d '\r\n' < "$_porchlight_root/VERSION")"
unset _porchlight_root
