#!/usr/bin/env bash
# bridge-from-json.sh — run the bridge mesh (N chains, M validators, K keepers)
# from a single JSON config.
#
#   bash scripts/bridge-from-json.sh [config.json]                  # generate + start
#   bash scripts/bridge-from-json.sh [config.json] --generate-only  # just write the TOMLs
#   bash scripts/bridge-from-json.sh [config.json] --stop           # tear the stack down
#   bash scripts/bridge-from-json.sh [config.json] --status         # what is alive
#
# Default config: config/bridge.config.json (field reference: config/README.md).
# Every chain listed bridges to every other one (full mesh, both directions);
# each validator watches all the chains it is given as sources and attests
# refunds on all destinations, and each keeper delivers claims on its targets.
#
# The generated validator/keeper/indexer TOMLs land in runtime.run_dir, so you
# can inspect exactly what each process was handed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="config/bridge.config.json"
MODE=start

for arg in "$@"; do
  case "$arg" in
    --generate-only) MODE=generate ;;
    --stop)          MODE=stop ;;
    --status)        MODE=status ;;
    -h|--help)       sed -n '2,18p' "$0"; exit 0 ;;
    -*)              echo "unknown flag: $arg" >&2; exit 1 ;;
    *)               CONFIG="$arg" ;;
  esac
done
[[ "$CONFIG" = /* ]] || CONFIG="$ROOT/$CONFIG"
[[ -f "$CONFIG" ]] || { echo "config not found: $CONFIG" >&2; exit 1; }

say()  { printf '\n\033[1;36m=== %s ===\033[0m\n' "$*"; }
info() { printf '  %s\n' "$*"; }
warn() { printf '\033[1;33m  ! %s\033[0m\n' "$*"; }
die()  { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "'$1' not found on PATH (needed for: $2)"; }
need jq "reading the JSON config"

j()  { jq -r "$1" "$CONFIG"; }
jr() { jq -r "$1 // empty" "$CONFIG"; }

NAME="$(j '.name')"
RUN_DIR="$(jr '.runtime.run_dir')"; RUN_DIR="${RUN_DIR:-/tmp/bridge-json-run}"
BIN_DIR="$(jr '.runtime.bin_dir')"; BIN_DIR="${BIN_DIR:-target/debug}"
[[ "$BIN_DIR" = /* ]] || BIN_DIR="$ROOT/$BIN_DIR"
LOG_LEVEL="$(jr '.runtime.log_level')"; LOG_LEVEL="${LOG_LEVEL:-info}"
PIDS="$RUN_DIR/pids"

pg_enabled() { [[ "$(j '.database.docker.enabled')" == "true" ]]; }
PG_NAME="$(jr '.database.docker.container')"; PG_NAME="${PG_NAME:-bridge-json-pg}"

# --- stop / status ---------------------------------------------------------
if [[ "$MODE" == "stop" ]]; then
  say "stopping $NAME"
  if [[ -f "$PIDS" ]]; then
    # pid first, then a match pattern: `setsid` may fork, in which case the
    # recorded pid is the wrapper and the service itself outlives the kill.
    while IFS=$'\t' read -r pid name pattern; do
      [[ -n "${pid:-}" ]] || continue
      kill "$pid" 2>/dev/null || true
      [[ -n "${pattern:-}" ]] && pkill -f -- "$pattern" 2>/dev/null || true
      info "stopped $name"
    done < "$PIDS"
    sleep 1
    while IFS=$'\t' read -r pid _ pattern; do
      kill -9 "$pid" 2>/dev/null || true
      [[ -n "${pattern:-}" ]] && pkill -9 -f -- "$pattern" 2>/dev/null || true
    done < "$PIDS"
    rm -f "$PIDS"
  else
    info "no pid file at $PIDS"
  fi
  # The Postgres VOLUME is kept: it holds signatures, transfer history and the
  # indexer cursors. Validators resume from file cursors and never re-sign blocks
  # they already scanned, so wiping it strands anything in flight. Remove it by
  # hand (`docker volume rm …`) only when a clean slate is really what you want.
  if pg_enabled && command -v docker >/dev/null 2>&1; then
    docker rm -f "$PG_NAME" >/dev/null 2>&1 && info "removed container $PG_NAME (volume kept)" || true
  fi
  exit 0
fi
if [[ "$MODE" == "status" ]]; then
  say "$NAME"
  [[ -f "$PIDS" ]] || { info "not running (no $PIDS)"; exit 0; }
  while IFS=$'\t' read -r pid name pattern; do
    if kill -0 "$pid" 2>/dev/null || pgrep -f -- "$pattern" >/dev/null 2>&1
    then printf '  %-22s up   (pid %s)\n' "$name" "$pid"
    else printf '  %-22s DOWN (pid %s)\n' "$name" "$pid"; fi
  done < "$PIDS"
  exit 0
fi

# ---------------------------------------------------------------------------
# validate
# ---------------------------------------------------------------------------
say "validating $CONFIG"
# per-chain field with fallback to .defaults
cf() { # $1 chain_id, $2 field
  local v; v="$(jq -r ".chains[] | select(.chain_id == $1) | .$2 // empty" "$CONFIG")"
  [[ -n "$v" && "$v" != "null" ]] && { echo "$v"; return; }
  jq -r ".defaults.$2 // empty" "$CONFIG"
}
cbool() { # $1 chain_id, $2 field — false is a real value, so `// empty` is wrong here
  local v; v="$(jq -r ".chains[] | select(.chain_id == $1) | .$2" "$CONFIG")"
  [[ "$v" == "null" ]] && v="$(jq -r ".defaults.$2 // false" "$CONFIG")"
  echo "$v"
}
# "all" or [id, id] -> chain ids, intersected with a role (source/destination)
select_chains() { # $1 jq path to the selector, $2 role field
  local sel; sel="$(jq -c "$1" "$CONFIG")"
  for cid in "${CHAIN_IDS[@]}"; do
    [[ "$(cbool "$cid" "$2")" == "false" ]] && continue
    if [[ "$sel" == '"all"' || "$sel" == "null" ]]; then echo "$cid"
    else jq -e --argjson c "$cid" 'index($c) != null' <<<"$sel" >/dev/null && echo "$cid"; fi
  done
}
# [signer] / [keeper] body from a JSON signer object
emit_signer() { # $1 jq path
  local any=false k
  for k in private_key private_key_env keystore keystore_password keystore_password_env keystore_password_file; do
    local v; v="$(jq -r "$1.$k // empty" "$CONFIG")"
    [[ -n "$v" ]] || continue
    echo "$k = \"$v\""; any=true
  done
  $any || die "signer at $1 has no key source (private_key / private_key_env / keystore)"
}

THRESHOLD="$(j '.threshold')"
mapfile -t CHAIN_IDS < <(j '.chains[].chain_id')
(( ${#CHAIN_IDS[@]} >= 1 )) || die ".chains is empty"
(( ${#CHAIN_IDS[@]} >= 2 )) || warn "only one chain configured — nothing to bridge to"
[[ "$(printf '%s\n' "${CHAIN_IDS[@]}" | sort | uniq -d)" == "" ]] || die "duplicate chain_id in .chains"

dup_names="$(j '[.validators[]?.name] + [.keepers[]?.name] | group_by(.) | map(select(length > 1) | .[0]) | join(", ")')"
[[ -z "$dup_names" ]] || die "duplicate validator/keeper name: $dup_names (names key the generated configs)"

VAL_COUNT="$(j '[.validators[] | select(.enabled != false)] | length')"
(( VAL_COUNT >= 1 )) || die "no enabled validators"
(( THRESHOLD >= 1 && THRESHOLD <= VAL_COUNT )) || die "threshold ($THRESHOLD) must be 1..$VAL_COUNT (enabled validators)"
(( THRESHOLD * 2 > VAL_COUNT )) || warn "threshold $THRESHOLD of $VAL_COUNT is not a strict majority"

for cid in "${CHAIN_IDS[@]}"; do
  gate="$(jr ".chains[] | select(.chain_id == $cid) | .gate")"
  [[ "$gate" =~ ^0x[0-9a-fA-F]{40}$ ]] || die "chain $cid has no gate address — run scripts/deploy-from-json.sh first"
  [[ "$gate" =~ ^0x0{40}$ ]] && die "chain $cid gate is the zero address — run scripts/deploy-from-json.sh first"
  # Fail closed exactly where the Rust services do, but with the chain named up
  # front instead of after half the stack is already running. Signing an event at
  # the chain tip lets a reorg erase the deposit after the destination paid out.
  bc="$(cf "$cid" block_confirmation)"; bc="${bc:-0}"
  az="$(cbool "$cid" allow_zero_confirmation)"
  if [[ "$bc" == "0" && "$az" != "true" ]]; then
    die "chain $cid: block_confirmation = 0 without allow_zero_confirmation. Set a finality buffer above the chain's reorg depth, or opt in ONLY for an instant-final dev chain (anvil)."
  fi
done

SOLANA_ON="$(j '.solana.enabled // false')"
if [[ "$SOLANA_ON" == "true" ]]; then
  SOL_CHAIN_ID="$(j '.solana.chain_id')"
  SOL_PROGRAM="$(jr '.solana.program_id')"
  SOL_COMMITMENT="$(j '.solana.commitment')"
  SOL_BIN="$(jr '.solana.bin')"; SOL_BIN="${SOL_BIN:-crates/solana-relayer/target/debug/solana-relayer}"
  [[ "$SOL_BIN" = /* ]] || SOL_BIN="$ROOT/$SOL_BIN"
  [[ -n "$SOL_PROGRAM" ]] || die "solana.enabled but no solana.program_id — run scripts/deploy-from-json.sh first"

  # Solana's finality control is the COMMITMENT level, not a block count. Signing
  # a Sent that a fork later discards is the same double-spend the EVM side's
  # block_confirmation defends against, so fail closed here exactly as the
  # relayer does — just earlier, and by name.
  case "$SOL_COMMITMENT" in
    finalized) ;;
    confirmed|processed)
      [[ "$(j '.solana.allow_unfinalized')" == "true" ]] \
        || die "solana.commitment = \"$SOL_COMMITMENT\" can be rolled back by a fork, so the relayer would sign a Sent whose deposit never settles. Use \"finalized\", or set solana.allow_unfinalized = true ONLY for a local test validator." ;;
    *) die "unknown solana.commitment: $SOL_COMMITMENT" ;;
  esac

  SOL_COUNT="$(j '[.solana.relayers[] | select(.enabled != false)] | length')"
  # Solana `Sent` events are signed ONLY by relayers — the EVM validators never
  # scan Solana — so a gate with threshold T needs at least T relayers, each
  # holding a DISTINCT validator key. Run fewer and Solana-origin transfers stall
  # below quorum, silently.
  (( SOL_COUNT >= THRESHOLD )) || die "solana.relayers has $SOL_COUNT enabled, threshold is $THRESHOLD — Solana-origin transfers would never reach quorum (the EVM validators do not scan Solana)"
  dup_keys="$(j '[.solana.relayers[] | select(.enabled != false) | (.signer.private_key_env // .signer.private_key)] | group_by(.) | map(select(length > 1) | .[0]) | length')"
  [[ "$dup_keys" == "0" ]] || die "two solana.relayers share a signing key — the quorum would count one key twice"
  deliverers="$(j '[.solana.relayers[] | select(.enabled != false and .deliver == true)] | length')"
  (( deliverers <= 1 )) || warn "$deliverers relayers have deliver = true; they submit the same claims from different payers and will race"
  (( deliverers >= 1 )) || warn "no relayer has deliver = true — nothing delivers EVM->Solana claims (signing still works)"
  for kp in $(j '.solana.relayers[] | select(.enabled != false and .deliver == true) | .payer_keypair // empty'); do
    [[ "$kp" = /* ]] || kp="$ROOT/$kp"
    [[ -f "$kp" ]] || die "relayer payer_keypair not found: $kp"
  done
fi

STORE_URL="$(jr '.sig_store.url')"; STORE_URL="${STORE_URL:-http://127.0.0.1:8080}"
STORE_BIND="$(jr '.sig_store.bind')"; STORE_BIND="${STORE_BIND:-127.0.0.1:8080}"
DATABASE_URL="$(jr '.database.url')"
[[ -n "$DATABASE_URL" ]] || die ".database.url is required (the sig-store and indexer are Postgres-backed)"

export RUST_LOG="$LOG_LEVEL"
export DATABASE_URL
mkdir -p "$RUN_DIR"

# ---------------------------------------------------------------------------
# generate configs
# ---------------------------------------------------------------------------
say "generating configs in $RUN_DIR"

REFUND_ON="$(j '.refund.enabled')"
mapfile -t REFUND_DESTS < <(select_chains '.refund.destinations' destination)

VAL_FILES=() VAL_NAMES=()
for idx in $(j '[.validators[] | select(.enabled != false)] | to_entries[].key'); do
  vpath=".validators[] | select(.enabled != false)"
  vname="$(jq -r "[$vpath] | .[$idx].name" "$CONFIG")"
  vjson=".validators[] | select(.name == \"$vname\")"
  cfg="$RUN_DIR/validator-$vname.toml"
  {
    for cid in $(select_chains "$(printf '(%s).sources' "$vjson")" source); do
      rpcs="$(jq -c ".chains[] | select(.chain_id == $cid) | .rpcs" "$CONFIG")"
      echo "[[sources]]"
      echo "chain_id = $cid"
      echo "rpcs = $rpcs"
      echo "gate = \"$(cf "$cid" gate)\""
      echo "start_block = $(cf "$cid" start_block)"
      echo "block_confirmation = $(cf "$cid" block_confirmation)"
      echo "allow_zero_confirmation = $(cbool "$cid" allow_zero_confirmation)"
      echo "poll_interval_ms = $(cf "$cid" poll_interval_ms)"
      echo "max_block_range = $(cf "$cid" max_block_range)"
      echo "state_file = \"$RUN_DIR/validator-$vname-$cid.json\""
      echo
    done
    echo "[signer]"
    emit_signer "($vjson).signer"
    echo
    echo "[store]"
    echo "url = \"$STORE_URL\""
    if [[ "$(jq -r "($vjson).api.enabled // false" "$CONFIG")" == "true" ]]; then
      echo
      echo "[api]"
      echo "bind = \"$(jq -r "($vjson).api.bind" "$CONFIG")\""
      tok="$(jq -r "($vjson).api.token // empty" "$CONFIG")"
      [[ -n "$tok" ]] && echo "token = \"$tok\""
    fi
    if [[ "$REFUND_ON" == "true" ]]; then
      # No [refund] block => this validator never votes on cancels/refunds, and
      # stranded transfers stay stranded. That is the safe default: a node that
      # cannot read the destination chain must not have an opinion on delivery.
      echo
      echo "[refund]"
      echo "timeout_secs = $(j '.refund.timeout_secs')"
      echo "poll_interval_ms = $(j '.refund.poll_interval_ms')"
      echo "block_confirmation = $(j '.refund.block_confirmation')"
      echo "allow_zero_confirmation = $(j '.refund.allow_zero_confirmation')"
      for cid in "${REFUND_DESTS[@]}"; do
        echo
        echo "[[refund.destinations]]"
        echo "chain_id = $cid"
        echo "rpcs = $(jq -c ".chains[] | select(.chain_id == $cid) | .rpcs" "$CONFIG")"
        echo "gate = \"$(cf "$cid" gate)\""
      done
    fi
  } > "$cfg"
  VAL_FILES+=("$cfg"); VAL_NAMES+=("$vname")
  info "validator $vname -> $cfg"
done

KEEP_FILES=() KEEP_NAMES=()
for idx in $(j '[.keepers[] | select(.enabled != false)] | to_entries[].key'); do
  kname="$(jq -r "[.keepers[] | select(.enabled != false)] | .[$idx].name" "$CONFIG")"
  kjson=".keepers[] | select(.name == \"$kname\")"
  poll="$(jq -r "($kjson).poll_interval_ms // 1000" "$CONFIG")"
  cfg="$RUN_DIR/keeper-$kname.toml"
  {
    for cid in $(select_chains "$(printf '(%s).targets' "$kjson")" destination); do
      echo "[[targets]]"
      echo "chain_id = $cid"
      echo "rpc = \"$(jq -r ".chains[] | select(.chain_id == $cid) | .rpcs[0]" "$CONFIG")\""
      echo "gate = \"$(cf "$cid" gate)\""
      echo "poll_interval_ms = $poll"
      echo
    done
    echo "[keeper]"
    emit_signer "($kjson).signer"
    echo
    echo "[store]"
    echo "url = \"$STORE_URL\""
    if [[ "$REFUND_ON" == "true" ]]; then
      # Refunds pay out where the funds were LOCKED, i.e. on the source chain —
      # hence their own blocks, separate from the claim targets.
      for cid in $(select_chains "$(printf '(%s).refund_sources' "$kjson")" source); do
        echo
        echo "[[sources]]"
        echo "chain_id = $cid"
        echo "rpc = \"$(jq -r ".chains[] | select(.chain_id == $cid) | .rpcs[0]" "$CONFIG")\""
        echo "gate = \"$(cf "$cid" gate)\""
        echo "poll_interval_ms = $poll"
      done
    fi
  } > "$cfg"
  KEEP_FILES+=("$cfg"); KEEP_NAMES+=("$kname")
  info "keeper $kname -> $cfg"
done
(( ${#KEEP_FILES[@]} >= 1 )) || warn "no enabled keepers — signatures will collect but nothing will claim on-chain"

SOL_FILES=() SOL_NAMES=()
if [[ "$SOLANA_ON" == "true" ]]; then
  for idx in $(j '[.solana.relayers[] | select(.enabled != false)] | to_entries[].key'); do
    rname="$(jq -r "[.solana.relayers[] | select(.enabled != false)] | .[$idx].name" "$CONFIG")"
    rjson=".solana.relayers[] | select(.name == \"$rname\")"
    cfg="$RUN_DIR/solana-relayer-$rname.toml"
    {
      echo "[source]"
      echo "chain_id = $SOL_CHAIN_ID"
      echo "rpc = \"$(j '.solana.rpc')\""
      echo "program_id = \"$SOL_PROGRAM\""
      echo "commitment = \"$SOL_COMMITMENT\""
      echo "allow_unfinalized = $(j '.solana.allow_unfinalized')"
      echo "poll_interval_ms = $(j '.solana.poll_interval_ms')"
      echo "max_batch = $(j '.solana.max_batch')"
      # Its own cursor file: two relayers sharing one would resume from each
      # other's position and skip signatures neither has signed.
      echo "state_file = \"$RUN_DIR/solana-relayer-$rname-state.json\""
      echo
      echo "[signer]"
      # secp256k1, and the SAME key this validator uses on the EVM side: one
      # validator set attests for both VMs. No keystore support here — the
      # relayer only reads `private_key` / `private_key_env`.
      kenv="$(jq -r "($rjson).signer.private_key_env // empty" "$CONFIG")"
      key="$(jq -r "($rjson).signer.private_key // empty" "$CONFIG")"
      if [[ -n "$kenv" ]]; then echo "private_key_env = \"$kenv\""
      elif [[ -n "$key" ]]; then echo "private_key = \"$key\""
      else die "solana relayer $rname has no signing key"; fi
      echo
      echo "[store]"
      echo "url = \"$STORE_URL\""
      echo "token_env = \"SIG_STORE_VALIDATOR_TOKEN\""
      if [[ "$(jq -r "($rjson).deliver // false" "$CONFIG")" == "true" ]]; then
        # The claim-submitting half (EVM -> Solana). Absent => this process only
        # SIGNS, which is a valid split: a validator need not be a keeper.
        kp="$(jq -r "($rjson).payer_keypair" "$CONFIG")"
        [[ "$kp" = /* ]] || kp="$ROOT/$kp"
        echo
        echo "[target]"
        echo "payer_keypair = \"$kp\""
        echo "poll_interval_ms = $(jq -r "($rjson).poll_interval_ms // 2000" "$CONFIG")"
      fi
    } > "$cfg"
    SOL_FILES+=("$cfg"); SOL_NAMES+=("$rname")
    info "solana relayer $rname -> $cfg"
  done
fi

IDX_CFG=""
if [[ "$(j '.indexer.enabled')" == "true" ]]; then
  IDX_CFG="$RUN_DIR/indexer.toml"
  {
    echo "database_url = \"$DATABASE_URL\""
    echo "refund_timeout_secs = $(j '.indexer.refund_timeout_secs')"
    echo "sweep_interval_secs = $(j '.indexer.sweep_interval_secs')"
    for cid in $(select_chains '.indexer.chains' source); do
      echo
      echo "[[chains]]"
      echo "chain_id = $cid"
      echo "rpc = \"$(jq -r ".chains[] | select(.chain_id == $cid) | .rpcs[0]" "$CONFIG")\""
      echo "gate = \"$(cf "$cid" gate)\""
      pool="$(jr ".chains[] | select(.chain_id == $cid) | .pool")"; [[ -n "$pool" ]] && echo "pool = \"$pool\""
      router="$(jr ".chains[] | select(.chain_id == $cid) | .router")"; [[ -n "$router" ]] && echo "router = \"$router\""
      echo "start_block = $(cf "$cid" start_block)"
      echo "block_confirmation = $(cf "$cid" block_confirmation)"
      echo "allow_zero_confirmation = $(cbool "$cid" allow_zero_confirmation)"
      echo "poll_interval_ms = $(cf "$cid" poll_interval_ms)"
      echo "max_block_range = $(cf "$cid" max_block_range)"
    done
  } > "$IDX_CFG"
  info "indexer -> $IDX_CFG"
fi

# registry the graphql API serves to the UI
REG_JSON="$RUN_DIR/chains.json"
jq '[ .chains[] | {chain_id, name, rpc_url: .rpcs[0], gate,
                   token: ((.tokens // [])[0].address // null),
                   tokens: (.tokens // []),
                   router} ]' "$CONFIG" > "$REG_JSON"
if [[ "$SOLANA_ON" == "true" && "$(j '.solana.include_in_registry')" == "true" ]]; then
  # No rpc_url/gate: the GraphQL API registers those for on-chain `executed`
  # lookups, and it speaks EVM JSON-RPC only. Listed, not polled.
  tmp="$(mktemp)"
  jq --slurpfile c <(jq '{solana}' "$CONFIG") '
    . + [{ chain_id: $c[0].solana.chain_id, name: $c[0].solana.name, rpc_url: null, gate: null,
           token: ($c[0].solana.tokens[0].mint // null),
           tokens: [$c[0].solana.tokens[] | select(.mint != null) | {symbol, address: .mint}],
           router: null }]' "$REG_JSON" > "$tmp" && mv "$tmp" "$REG_JSON"
fi
info "registry -> $REG_JSON"

[[ "$MODE" == "generate" ]] && { say "generated (not started)"; exit 0; }

# ---------------------------------------------------------------------------
# start
# ---------------------------------------------------------------------------
spawn() { # $1 command, $2 log name, $3 display name, $4 pattern that matches only this process
  setsid bash -c "exec $1" >"$RUN_DIR/$2" 2>&1 </dev/null &
  local pid=$!
  disown || true
  printf '%s\t%s\t%s\n' "$pid" "$3" "$4" >> "$PIDS"
}

if [[ -f "$PIDS" ]]; then
  say "stopping the previous run"
  bash "$0" "$CONFIG" --stop >/dev/null 2>&1 || true
  sleep 1
fi
: > "$PIDS"

if [[ "$(j '.runtime.build')" == "true" ]]; then
  say "building rust services"
  pkgs=(-p sig-store -p validator -p keeper -p graphql-api)
  [[ -n "$IDX_CFG" ]] && pkgs+=(-p indexer)
  ( cd "$ROOT" && cargo build "${pkgs[@]}" ) || die "cargo build failed"
fi
for b in sig-store validator keeper graphql-api; do
  [[ -x "$BIN_DIR/$b" ]] || die "missing binary $BIN_DIR/$b (set runtime.build = true, or point runtime.bin_dir at your release build)"
done

if pg_enabled; then
  need docker "the Postgres-backed sig-store"
  say "starting Postgres ($PG_NAME)"
  PG_PORT="$(j '.database.docker.port')"; PG_VOL="$(j '.database.docker.volume')"
  docker rm -f "$PG_NAME" >/dev/null 2>&1 || true
  docker volume create "$PG_VOL" >/dev/null 2>&1 || true
  docker run -d --name "$PG_NAME" \
    -e POSTGRES_USER="$(j '.database.docker.user')" \
    -e POSTGRES_PASSWORD="$(j '.database.docker.password')" \
    -e POSTGRES_DB="$(j '.database.docker.db')" \
    -v "$PG_VOL:/var/lib/postgresql/data" -p "$PG_PORT:5432" \
    "$(j '.database.docker.image')" >/dev/null || die "could not start $PG_NAME"
  ok=false
  for _ in $(seq 1 60); do docker exec "$PG_NAME" pg_isready -U "$(j '.database.docker.user')" >/dev/null 2>&1 && { ok=true; break; }; sleep 0.5; done
  $ok || die "Postgres did not become ready"
  info "Postgres ready on :$PG_PORT"
fi

# Scoped sig-store credentials. With none set the store starts UNAUTHENTICATED:
# signatures, claim status and the allowlist all become writable by anything that
# can reach the port. One token per role, so a leak from one component cannot act
# as another.
rand_token() { openssl rand -hex 32 2>/dev/null || head -c32 /dev/urandom | od -An -tx1 | tr -d ' \n'; }
gen_if_unset="$(j '.sig_store.tokens.generate_if_unset')"
for role in VALIDATOR KEEPER READER ADMIN; do
  lower="$(tr 'A-Z' 'a-z' <<<"$role")"
  val="$(jr ".sig_store.tokens.$lower")"
  if [[ -z "$val" ]]; then
    [[ "$gen_if_unset" == "true" ]] || warn "sig_store.tokens.$lower unset and generate_if_unset=false — the store will run unauthenticated"
    [[ "$gen_if_unset" == "true" ]] && val="$(rand_token)"
  fi
  export "SIG_STORE_${role}_TOKEN=$val"
done
umask 077
{ for role in VALIDATOR KEEPER READER ADMIN; do
    v="SIG_STORE_${role}_TOKEN"; echo "$v=${!v}"
  done; } > "$RUN_DIR/tokens.env"

if [[ "$(j '.sig_store.enabled')" == "true" ]]; then
  say "starting sig-store ($STORE_URL)"
  # --bind on the command line (not the env var) so the process line identifies
  # THIS stack: the stop fallback matches on it, and a second stack on another
  # port is left alone. Tokens stay in the environment — a command line is
  # world-readable in /proc.
  spawn "$BIN_DIR/sig-store --bind $STORE_BIND" sig-store.log sig-store "sig-store --bind $STORE_BIND"
  ok=false
  for _ in $(seq 1 80); do curl -s "$STORE_URL/health" 2>/dev/null | grep -q ok && { ok=true; break; }; sleep 0.25; done
  $ok || die "sig-store did not come up (see $RUN_DIR/sig-store.log)"
  info "healthy; tokens in $RUN_DIR/tokens.env"
fi

say "starting ${#VAL_FILES[@]} validator(s)"
for i in "${!VAL_FILES[@]}"; do
  spawn "$BIN_DIR/validator ${VAL_FILES[$i]}" "validator-${VAL_NAMES[$i]}.log" "validator-${VAL_NAMES[$i]}" "${VAL_FILES[$i]}"
  info "${VAL_NAMES[$i]}"
done

say "starting ${#KEEP_FILES[@]} keeper(s)"
for i in "${!KEEP_FILES[@]}"; do
  spawn "$BIN_DIR/keeper ${KEEP_FILES[$i]}" "keeper-${KEEP_NAMES[$i]}.log" "keeper-${KEEP_NAMES[$i]}" "${KEEP_FILES[$i]}"
  info "${KEEP_NAMES[$i]}"
done

if (( ${#SOL_FILES[@]} )); then
  say "starting ${#SOL_FILES[@]} solana relayer(s)"
  if [[ "$(j '.solana.build')" == "true" ]]; then
    # Its own cargo project on purpose: solana-client pins zeroize <1.4 and alloy
    # needs ^1.5, so the relayer cannot live in the workspace with the EVM services.
    ( cd "$ROOT" && cargo build --manifest-path crates/solana-relayer/Cargo.toml --bin solana-relayer ) \
      || die "building solana-relayer failed"
  fi
  [[ -x "$SOL_BIN" ]] || die "missing $SOL_BIN (set solana.build = true, or point solana.bin at your build)"
  for i in "${!SOL_FILES[@]}"; do
    spawn "$SOL_BIN ${SOL_FILES[$i]}" "solana-relayer-${SOL_NAMES[$i]}.log" "solana-relayer-${SOL_NAMES[$i]}" "${SOL_FILES[$i]}"
    info "${SOL_NAMES[$i]}"
  done
fi

if [[ -n "$IDX_CFG" ]]; then
  say "starting indexer"
  spawn "$BIN_DIR/indexer $IDX_CFG" indexer.log indexer "$IDX_CFG"
fi

if [[ "$(j '.graphql.enabled')" == "true" ]]; then
  GQL_BIND="$(j '.graphql.bind')"
  say "starting graphql-api ($GQL_BIND)"
  args=(--bind "$GQL_BIND" --store-url "$STORE_URL" --threshold "$THRESHOLD" --chains-file "$REG_JSON")
  [[ "$(j '.graphql.allow_mutations')" == "true" ]] && args+=(--allow-mutations)
  for cid in "${CHAIN_IDS[@]}"; do
    args+=(--gate "$cid=$(jq -r ".chains[] | select(.chain_id == $cid) | .rpcs[0]" "$CONFIG"),$(cf "$cid" gate)")
  done
  if [[ "$(j '.graphql.swap.enabled')" == "true" ]]; then
    scid="$(j '.graphql.swap.chain_id')"
    args+=(--swap "$scid=$(jq -r ".chains[] | select(.chain_id == $scid) | .rpcs[0]" "$CONFIG"),$(j '.graphql.swap.pool'),$(j '.graphql.swap.from_block')")
  fi
  # No --db-url: graphql-api reads history through the sig-store on its reader
  # token. It is the only service meant to face the internet, so it holds no
  # database credential of its own.
  export GRAPHQL_MAX_BLOCK_RANGE="$(j '.defaults.max_block_range')"
  spawn "$BIN_DIR/graphql-api ${args[*]}" graphql-api.log graphql-api "$REG_JSON"
  ok=false
  for _ in $(seq 1 80); do curl -s "http://$GQL_BIND/health" >/dev/null 2>&1 && { ok=true; break; }; sleep 0.25; done
  $ok || die "graphql-api did not come up (see $RUN_DIR/graphql-api.log)"
  info "healthy"
fi

if [[ "$(j '.frontend.enabled // false')" == "true" ]]; then
  FE_DIR="$(jr '.frontend.dir')"; FE_DIR="${FE_DIR:-frontend}"
  [[ "$FE_DIR" = /* ]] || FE_DIR="$ROOT/$FE_DIR"
  FE_HOST="$(jr '.frontend.host')"; FE_HOST="${FE_HOST:-127.0.0.1}"
  FE_PORT="$(jr '.frontend.port')"; FE_PORT="${FE_PORT:-5173}"
  say "starting frontend (vite on :$FE_PORT)"
  # nvm installs are not on a non-login shell's PATH, so find the newest one
  # rather than failing on a node that is installed and simply not visible here.
  FE_NODE_BIN="$(jr '.frontend.node_bin')"
  if [[ -z "$FE_NODE_BIN" ]] && ! command -v node >/dev/null 2>&1; then
    FE_NODE_BIN="$(ls -d "$HOME"/.nvm/versions/node/*/bin 2>/dev/null | sort -V | tail -1 || true)"
  fi
  [[ -n "$FE_NODE_BIN" ]] && export PATH="$FE_NODE_BIN:$PATH"
  need node "the frontend"; need npm "the frontend"
  if [[ ! -d "$FE_DIR/node_modules" ]]; then
    [[ "$(j '.frontend.install')" == "true" ]] || die "no $FE_DIR/node_modules and frontend.install = false"
    ( cd "$FE_DIR" && npm install --no-audit --no-fund ) || die "npm install failed"
  fi
  # The UI talks to the API through vite's proxy, so it needs no CORS and no
  # public API port — the same wiring scripts/run.sh uses.
  export VITE_PROXY_TARGET="http://$(j '.graphql.bind')"
  ( cd "$FE_DIR" && spawn "npx vite --host $FE_HOST --port $FE_PORT --strictPort" web.log frontend "vite --host $FE_HOST --port $FE_PORT" )
  ok=false
  for _ in $(seq 1 80); do curl -s "http://127.0.0.1:$FE_PORT/" >/dev/null 2>&1 && { ok=true; break; }; sleep 0.3; done
  $ok && info "http://$FE_HOST:$FE_PORT" || warn "vite did not answer yet (see $RUN_DIR/web.log)"
fi

say "$NAME is up"
for cid in "${CHAIN_IDS[@]}"; do
  printf '  %-14s %s  gate %s\n' "$(jq -r ".chains[] | select(.chain_id == $cid) | .name" "$CONFIG")" \
    "$(jq -r ".chains[] | select(.chain_id == $cid) | .rpcs[0]" "$CONFIG")" "$(cf "$cid" gate)"
done
printf '  %-14s %s\n' "validators" "${VAL_NAMES[*]} (threshold $THRESHOLD)"
printf '  %-14s %s\n' "keepers"    "${KEEP_NAMES[*]:-none}"
[[ "$SOLANA_ON" == "true" ]] && printf '  %-14s %s  program %s (%s relayers, commitment %s)\n' \
  "$(j '.solana.name')" "$(j '.solana.rpc')" "$SOL_PROGRAM" "${#SOL_FILES[@]}" "$SOL_COMMITMENT"
printf '  %-14s %s\n' "sig-store"  "$STORE_URL"
[[ "$(j '.graphql.enabled')" == "true" ]] && printf '  %-14s http://%s  (GraphiQL at /)\n' "graphql" "$(j '.graphql.bind')"
[[ "$(j '.frontend.enabled // false')" == "true" ]] && printf '  %-14s http://%s:%s\n' "frontend" "$(j '.frontend.host')" "$(j '.frontend.port')"
echo
info "logs:   $RUN_DIR/*.log"
info "status: bash scripts/bridge-from-json.sh $CONFIG --status"
info "stop:   bash scripts/bridge-from-json.sh $CONFIG --stop"
