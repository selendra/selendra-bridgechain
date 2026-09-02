#!/usr/bin/env sh
# Preflight for this stack. Run it before `docker compose up -d`.
#
# It exists because of one non-obvious failure: docker compose file-secrets are
# bind-mounted with the HOST file's ownership and mode, and compose IGNORES the
# `uid`/`gid`/`mode` long-syntax fields outside swarm mode. The image runs as
# uid 10001 (`bridge`), so a keystore left at the usual 0600 root/you ownership
# is unreadable inside the container, and the only symptom is a restart loop
# logging:
#
#     Error: loading validator signer
#     Caused by: reading keystore_password_file /run/secrets/keystore_password
#                Permission denied (os error 13)
#
# The fix is host-side permissions, and there are two correct shapes. See below.
set -eu
cd "$(dirname "$0")"

fail=0
say() { printf '%s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; fail=1; }
ok()  { printf '  ok    %s\n' "$*"; }

say "preflight: $(basename "$PWD")"

[ -f .env ] || { bad ".env missing — cp .env.example .env and fill it in"; }
if [ -f .env ]; then
  mode=$(stat -c '%a' .env 2>/dev/null || stat -f '%Lp' .env)
  case "$mode" in
    600|400) ok ".env mode $mode" ;;
    *) bad ".env is mode $mode — it holds the sig-store token; chmod 600 .env" ;;
  esac
  # Every KEY= line with an empty value is an unfilled template slot.
  empties=$(grep -E '^[A-Z_]+=$' .env || true)
  [ -z "$empties" ] || bad "unfilled values in .env: $(echo "$empties" | tr -d '=' | tr '\n' ' ')"
fi

for f in configs/*.toml configs/*.json; do
  [ -e "$f" ] || continue
  found=1
done
[ "${found:-0}" = 1 ] || bad "no config in configs/ — cp configs/*.example to the name without .example, then edit"

# --- the secrets, and the uid-10001 rule ------------------------------------
dirmode=$(stat -c '%a' secrets 2>/dev/null || echo missing)
[ "$dirmode" = 700 ] || bad "secrets/ is mode $dirmode — chmod 700 secrets"

for f in secrets/keystore.json secrets/keystore-password; do
  if [ ! -f "$f" ]; then
    bad "$f missing — see secrets/README.md"
    continue
  fi
  uid=$(stat -c '%u' "$f" 2>/dev/null || stat -f '%u' "$f")
  mode=$(stat -c '%a' "$f" 2>/dev/null || stat -f '%Lp' "$f")
  if [ "$uid" = 10001 ]; then
    # Shape A (strictest): owned by the container user, unreadable by anyone else.
    case "$mode" in
      600|400) ok "$f uid 10001 mode $mode" ;;
      *) bad "$f is uid 10001 but mode $mode — chmod 600 $f" ;;
    esac
  else
    # Shape B (no root needed): the 0700 directory is what keeps other host
    # users out; the file itself must be world-readable for uid 10001 to read
    # it through the bind mount, which does not re-check directory traversal.
    case "$mode" in
      644|444) ok "$f mode $mode inside a 0700 dir" ;;
      *) bad "$f is uid $uid mode $mode — the container runs as uid 10001 and cannot read it.
        Pick one:
          A) sudo chown 10001:10001 $f && chmod 600 $f      (strictest)
          B) chmod 700 secrets && chmod 644 $f              (no root needed)" ;;
    esac
  fi
done

echo
if [ "$fail" = 0 ]; then
  say "preflight passed — docker compose up -d"
else
  say "preflight FAILED — fix the above before starting"
  exit 1
fi
