# Source this file, then `porchlight_local_env <repo root>`: loads the gitignored machine profile <root>/.env
# (see .env.example). Lines are PORCHLIGHT_KEY=value with a literal value (no quotes or expansion); variables already
# set in the environment win, so `PORCHLIGHT_DEPLOY_HOST=other scripts/deploy-server.sh ...` still overrides.
# POSIX sh, bash and zsh: scripts/env.sh is sourced into interactive shells.
porchlight_local_env() {
  [ -f "$1/.env" ] || return 0
  while IFS= read -r _pl_line || [ -n "$_pl_line" ]; do
    case $_pl_line in
      '' | '#'*) ;;
      PORCHLIGHT_*=*)
        _pl_key=${_pl_line%%=*}
        case $_pl_key in
          *[!A-Z0-9_]*) printf 'porchlight: ignoring invalid key in %s/.env: %s\n' "$1" "$_pl_key" >&2 ;;
          *)
            eval "_pl_set=\${$_pl_key+x}"
            [ -n "$_pl_set" ] || export "$_pl_line" ;;
        esac ;;
      *) printf 'porchlight: ignoring a line in %s/.env (want PORCHLIGHT_KEY=value)\n' "$1" >&2 ;;
    esac
  done < "$1/.env"
  unset _pl_line _pl_key _pl_set
}
