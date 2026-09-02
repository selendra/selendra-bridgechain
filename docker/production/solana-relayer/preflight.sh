#!/usr/bin/env sh
# Preflight for the Solana relayer. Run it before `docker compose up -d`.
#
# Same uid-10001 trap as the EVM validator/ and keeper/ stacks: docker compose
# bind-mounts file-secrets with the HOST file's ownership and IGNORES the
# `uid`/`gid`/`mode` long syntax outside swarm mode. The relayer image runs as
# uid 10001, so a payer keypair left at your-user:0600 is unreadable inside the
# container.
set -eu
cd "$(dirname "$0")"

fail=0
say() { printf '%s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; fail=1; }
warn(){ printf '  warn  %s\n' "$*"; }
ok()  { printf '  ok    %s\n' "$*"; }

say "preflight: solana-relayer"

[ -f .env ] || bad ".env missing — cp .env.example .env and fill it in"
if [ -f .env ]; then
  mode=$(stat -c '%a' .env 2>/dev/null || stat -f '%Lp' .env)
  case "$mode" in
    600|400) ok ".env mode $mode" ;;
    *) bad ".env is mode $mode — it holds the RAW VALIDATOR KEY (no keystore
        option exists here); chmod 600 .env" ;;
  esac
  empties=$(grep -E '^[A-Z_]+=$' .env || true)
  [ -z "$empties" ] || bad "unfilled values in .env: $(echo "$empties" | tr -d '=' | tr '\n' ' ')"
fi

cfg=configs/solana-relayer.toml
if [ ! -f "$cfg" ]; then
  bad "$cfg missing — copy ONE of:
          configs/solana-relayer.toml.example          (sign-only: validator role)
          configs/solana-relayer.deliver.toml.example  (also delivers: keeper role)
        to $cfg, then edit it"
else
  ok "$cfg present"

  # Fail-closed rule, mirrored here so it is caught before the container starts.
  if grep -qE '^[[:space:]]*allow_unfinalized[[:space:]]*=[[:space:]]*true' "$cfg"; then
    bad "allow_unfinalized = true — the relayer would sign Sent events a fork can
        still discard, so the destination could pay out against a deposit that
        never settles. Correct ONLY against a local test validator."
  elif grep -qE '^[[:space:]]*commitment[[:space:]]*=[[:space:]]*"finalized"' "$cfg"; then
    ok 'commitment = "finalized"'
  else
    warn "no explicit commitment in $cfg — it defaults to \"finalized\", which is correct"
  fi

  # An inline key defeats the point of private_key_env.
  if grep -qE '^[[:space:]]*private_key[[:space:]]*=' "$cfg"; then
    bad "$cfg has an inline private_key — use private_key_env = \"SOLANA_VALIDATOR_KEY\"
        instead; this file is mounted into the container and is easier to leak"
  fi

  # --- the payer keypair, only when [target] is configured -------------------
  if grep -qE '^[[:space:]]*\[target\]' "$cfg"; then
    say "  ..  [target] present: this relayer will DELIVER EVM->Solana claims"
    say "      start it with:  docker compose -f docker-compose.yml -f docker-compose.deliver.yml up -d"
    f=secrets/payer.json
    dirmode=$(stat -c '%a' secrets 2>/dev/null || echo missing)
    [ "$dirmode" = 700 ] || bad "secrets/ is mode $dirmode — chmod 700 secrets"
    if [ ! -f "$f" ]; then
      bad "$f missing — [target] needs a funded Solana keypair (see secrets/README.md)"
    else
      uid=$(stat -c '%u' "$f" 2>/dev/null || stat -f '%u' "$f")
      mode=$(stat -c '%a' "$f" 2>/dev/null || stat -f '%Lp' "$f")
      if [ "$uid" = 10001 ]; then
        case "$mode" in
          600|400) ok "$f uid 10001 mode $mode" ;;
          *) bad "$f is uid 10001 but mode $mode — chmod 600 $f" ;;
        esac
      else
        case "$mode" in
          644|444) ok "$f mode $mode inside a 0700 dir" ;;
          *) bad "$f is uid $uid mode $mode — the container runs as uid 10001 and cannot read it.
        Pick one:
          A) sudo chown 10001:10001 $f && chmod 600 $f      (strictest)
          B) chmod 700 secrets && chmod 644 $f              (no root needed)" ;;
        esac
      fi
    fi
  else
    ok "no [target]: sign-only (validator role), no payer keypair needed"
  fi
fi

echo
if [ "$fail" = 0 ]; then
  say "preflight passed"
else
  say "preflight FAILED — fix the above before starting"
  exit 1
fi
