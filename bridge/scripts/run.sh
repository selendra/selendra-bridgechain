#!/usr/bin/env bash
# run.sh — one-command launcher for the whole bridge + swap stack, driven by a
# config file (scripts/run.config by default).
#
# Brings up, per the config:
#   * N chains (local anvil, or your own RPCs) — 2, 3, 4, …
#   * Gate + token on each, wired full-mesh for bidirectional bridging between
#     every pair of chains, + an optional SwapPool on one chain
#   * Postgres + sig-store + M validators (threshold) + keeper
#   * optional indexer (history + refund eligibility) and the two-phase refund path
#   * graphql-api (backend) + the React frontend (vite)
#
# Usage:
#   bash scripts/run.sh [config-file]     # default: scripts/run.config
#   bash scripts/stop.sh [config-file]    # tear it all down
#
# Re-running is idempotent: it stops the previous run first.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS="$ROOT/contracts"
FRONTEND="$ROOT/frontend"
CONFIG="${1:-$ROOT/scripts/run.config}"

[[ -f "$CONFIG" ]] || { echo "config file not found: $CONFIG" >&2; exit 1; }
# shellcheck disable=SC1090
source "$CONFIG"

# --- resolve toolchain onto PATH (auto-detect newest nvm node if unset) ---
if [[ -z "${NODE_BIN:-}" ]]; then
  NODE_BIN="$(ls -d "$HOME"/.nvm/versions/node/*/bin 2>/dev/null | sort -V | tail -1 || true)"
fi
export PATH="${NODE_BIN:+$NODE_BIN:}${FOUNDRY_BIN:-}:${CARGO_BIN:-}:$PATH"

RUN_DIR="${RUN_DIR:-/tmp/bridge-run}"
mkdir -p "$RUN_DIR"
ADDR_ENV="$RUN_DIR/addresses.env"     # generated deploy addresses (for the summary + reruns)
REG_JSON="$RUN_DIR/chains.json"       # generated registry the frontend reads via graphql

STORE_URL="http://$BIND_HOST:$STORE_PORT"
GQL_BIND="$BIND_HOST:$GQL_PORT"

say()  { printf '\n\033[1;36m=== %s ===\033[0m\n' "$*"; }
info() { printf '  %s\n' "$*"; }
die()  { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

# Long-lived service, detached so it survives this shell. $1 command, $2 logfile.
spawn() { setsid bash -c "exec $1" >"$RUN_DIR/$2" 2>&1 </dev/null & disown || true; }
need()  { command -v "$1" >/dev/null 2>&1 || die "'$1' not found on PATH (needed for: $2)"; }

(( BASH_VERSINFO[0] >= 4 )) || die "bash 4+ required (found $BASH_VERSION); the ASSETS map needs associative arrays"

# back-compat: an old single DEPLOY flag seeds all three sub-flags if the new
# ones aren't set.
if [[ -n "${DEPLOY:-}" ]]; then
  DEPLOY_TOKENS="${DEPLOY_TOKENS:-$DEPLOY}"; DEPLOY_BRIDGE="${DEPLOY_BRIDGE:-$DEPLOY}"; DEPLOY_SWAP="${DEPLOY_SWAP:-$DEPLOY}"
fi
DEPLOY_TOKENS="${DEPLOY_TOKENS:-true}"; DEPLOY_BRIDGE="${DEPLOY_BRIDGE:-true}"; DEPLOY_SWAP="${DEPLOY_SWAP:-true}"

# ---------------------------------------------------------------------------
# parse CHAINS -> CID CNAME CRPC CGATE   (gate optional 4th field)
# ---------------------------------------------------------------------------
CID=() CNAME=() CRPC=() CGATE=()
declare -A CIDX                                   # chain_id -> array index
for entry in "${CHAINS[@]}"; do
  IFS='|' read -r cid cname crpc cgate <<<"$entry"
  cid="${cid// /}"; cname="$(echo "$cname" | sed 's/^ *//;s/ *$//')"; crpc="${crpc// /}"; cgate="${cgate// /}"
  [[ -n "$cid" && -n "$crpc" ]] || die "bad CHAINS entry: '$entry' (need chain_id|name|rpc)"
  CIDX[$cid]=${#CID[@]}
  CID+=("$cid"); CNAME+=("${cname:-chain $cid}"); CRPC+=("$crpc"); CGATE+=("$cgate")
done
N=${#CID[@]}
(( N >= 2 )) || die "CHAINS needs at least 2 chains (got $N)"
SWAP_CHAIN="${SWAP_CHAIN:-${CID[0]}}"

# ---------------------------------------------------------------------------
# parse ASSETS -> ATOKEN["<sym>|<chain_id>"] = token ; ACHAINS["<sym>"]="c1 c2"
# ---------------------------------------------------------------------------
ASYMS=()
declare -A ATOKEN ACHAINS
for entry in "${ASSETS[@]:-}"; do
  [[ -z "$entry" ]] && continue
  IFS='|' read -r sym rest <<<"$entry"
  sym="${sym// /}"; [[ -n "$sym" ]] || die "bad ASSETS entry (empty symbol): '$entry'"
  ASYMS+=("$sym")
  IFS='|' read -ra pairs <<<"$rest"
  for pv in "${pairs[@]}"; do
    pv="${pv// /}"; [[ -z "$pv" ]] && continue
    cid="${pv%%:*}"; tok="${pv#*:}"
    [[ -n "${CIDX[$cid]+x}" ]] || die "asset $sym lists chain $cid, which is not in CHAINS"
    ATOKEN["$sym|$cid"]="$tok"
    ACHAINS[$sym]="${ACHAINS[$sym]:-} $cid"
  done
  [[ -n "${ACHAINS[$sym]:-}" ]] || die "asset $sym has no chains"
done
(( ${#ASYMS[@]} >= 1 )) || die "ASSETS needs at least one asset"

# ---------------------------------------------------------------------------
# 0. preflight
# ---------------------------------------------------------------------------
say "preflight"
need cargo "building the Rust services"
need node  "the frontend"; need npm "the frontend"
if [[ "$LOCAL_ANVIL" == "true" || "$DEPLOY_TOKENS" == "true" || "$DEPLOY_BRIDGE" == "true" || "$DEPLOY_SWAP" == "true" ]]; then
  need anvil "local chains / deploys"; need cast "chain ops"; need forge "contract deploys"
else
  need cast "chain reads"
fi
[[ "$PG_DOCKER" == "true" ]] && need docker "the Postgres-backed sig-store"
info "config: $CONFIG"
info "chains: $N  ($(IFS=,; echo "${CID[*]}"))"
info "assets: ${ASYMS[*]}"
info "deploy: tokens=$DEPLOY_TOKENS bridge=$DEPLOY_BRIDGE swap=$DEPLOY_SWAP"
info "validators: ${#VALIDATOR_KEYS[@]}  threshold: $THRESHOLD"
info "features: swap=$ENABLE_SWAP indexer=$ENABLE_INDEXER refund=$ENABLE_REFUND"

# derive validator addresses from their keys
VALIDATOR_ADDRS=()
for k in "${VALIDATOR_KEYS[@]}"; do VALIDATOR_ADDRS+=("$(cast wallet address --private-key "$k")"); done
DEPLOYER_ADDR="$(cast wallet address --private-key "$DEPLOYER_KEY")"
(( THRESHOLD >= 1 && THRESHOLD <= ${#VALIDATOR_KEYS[@]} )) || die "THRESHOLD must be 1..${#VALIDATOR_KEYS[@]}"

# index of the swap chain within the arrays
swap_idx=-1
for i in "${!CID[@]}"; do [[ "${CID[$i]}" == "$SWAP_CHAIN" ]] && swap_idx=$i; done
[[ "$ENABLE_SWAP" != "true" || $swap_idx -ge 0 ]] || die "SWAP_CHAIN=$SWAP_CHAIN is not in CHAINS"

# ---------------------------------------------------------------------------
# 1. stop any previous run
# ---------------------------------------------------------------------------
say "stopping any previous run"
bash "$ROOT/scripts/stop.sh" "$CONFIG" >/dev/null 2>&1 || true
sleep 1

# ---------------------------------------------------------------------------
# 2. build the Rust services we need
# ---------------------------------------------------------------------------
say "building rust services"
BUILD_PKGS=(-p sig-store -p validator -p keeper -p graphql-api)
[[ "$ENABLE_INDEXER" == "true" ]] && BUILD_PKGS+=(-p indexer)
( cd "$ROOT" && cargo build "${BUILD_PKGS[@]}" ) || die "cargo build failed"

# ---------------------------------------------------------------------------
# 3. chains (local anvil or external)
# ---------------------------------------------------------------------------
if [[ "$LOCAL_ANVIL" == "true" ]]; then
  say "booting $N anvil chain(s)"
  for i in "${!CID[@]}"; do
    port="${CRPC[$i]##*:}"
    spawn "anvil --chain-id ${CID[$i]} --port $port --host 127.0.0.1 --silent" "anvil-${CID[$i]}.log"
    info "${CNAME[$i]} (${CID[$i]}) on :$port"
  done
fi
for i in "${!CID[@]}"; do
  ok=false
  for _ in $(seq 1 60); do cast chain-id --rpc-url "${CRPC[$i]}" >/dev/null 2>&1 && { ok=true; break; }; sleep 0.25; done
  $ok || die "RPC not reachable: ${CRPC[$i]}"
  got="$(cast chain-id --rpc-url "${CRPC[$i]}")"
  [[ "$got" == "${CID[$i]}" ]] || die "RPC ${CRPC[$i]} reports chainId $got, config says ${CID[$i]}"
done

# ---------------------------------------------------------------------------
# 4. deploy tokens / bridge / swap (each independent) + wire the mesh
# ---------------------------------------------------------------------------
deployed_to() { grep deployedTo | grep -oE '0x[0-9a-fA-F]{40}' | head -1; }
csend() { cast send "$1" "$2" "${@:3}" >/dev/null; }
vlist="[$(IFS=,; echo "${VALIDATOR_ADDRS[*]}")]"      # "[0xV1,0xV2,...]"
MINT=1000000000000000000000000                        # 1,000,000e18
SWAP_POOL=""
need_forge=false
[[ "$DEPLOY_TOKENS" == "true" || "$DEPLOY_BRIDGE" == "true" || "$DEPLOY_SWAP" == "true" ]] && need_forge=true
if $need_forge; then ( cd "$CONTRACTS" && forge build >/dev/null ) || die "forge build failed"; fi
fc() { ( cd "$CONTRACTS" && forge create "$1" --rpc-url "$2" --private-key "$DEPLOYER_KEY" --broadcast --json "${@:3}" 2>/dev/null ) | deployed_to; }

# --- 4a. tokens: resolve every asset's per-chain token address ---
say "resolving asset tokens (deploy_tokens=$DEPLOY_TOKENS)"
for sym in "${ASYMS[@]}"; do
  for cid in ${ACHAINS[$sym]}; do
    i=${CIDX[$cid]}; tok="${ATOKEN[$sym|$cid]}"
    if [[ "$tok" == "auto" ]]; then
      [[ "$DEPLOY_TOKENS" == "true" ]] || die "asset $sym on chain $cid is 'auto' but DEPLOY_TOKENS=false — give a 0x address"
      addr=$(fc src/TestToken.sol:TestToken "${CRPC[$i]}" --constructor-args "$sym" "$sym")
      [[ "$addr" =~ ^0x ]] || die "token deploy failed: $sym on chain $cid"
      ATOKEN["$sym|$cid"]="$addr"
    elif [[ ! "$tok" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
      die "asset $sym on chain $cid: '$tok' is neither 'auto' nor a 0x address"
    fi
    info "$sym @ $cid = ${ATOKEN[$sym|$cid]}"
  done
done

# --- 4b. bridge: deploy Gates, then register + fund every asset's mesh ---
if [[ "$DEPLOY_BRIDGE" == "true" ]]; then
  say "deploying Gates (validators=${#VALIDATOR_ADDRS[@]}, threshold=$THRESHOLD)"
  for i in "${!CID[@]}"; do
    CGATE[$i]=$(fc src/Gate.sol:Gate "${CRPC[$i]}" --constructor-args "$vlist" "$THRESHOLD")
    [[ "${CGATE[$i]}" =~ ^0x ]] || die "gate deploy failed on chain ${CID[$i]}"
    info "${CNAME[$i]} gate=${CGATE[$i]}"
  done
else
  for i in "${!CID[@]}"; do
    [[ "${CGATE[$i]}" =~ ^0x ]] || die "DEPLOY_BRIDGE=false but chain ${CID[$i]} has no gate in CHAINS"
  done
fi

# Wire when anything fresh was deployed (setLocalToken is idempotent; extra
# liquidity is harmless on a test chain). Skip entirely for a pure existing stack.
if [[ "$DEPLOY_BRIDGE" == "true" || "$DEPLOY_TOKENS" == "true" ]]; then
  say "wiring asset meshes (liquidity + setLocalToken)"
  for sym in "${ASYMS[@]}"; do
    read -ra chs <<<"${ACHAINS[$sym]}"
    (( ${#chs[@]} >= 2 )) || { info "$sym is on <2 chains — spendable only, not bridgeable"; }
    for cid in "${chs[@]}"; do
      i=${CIDX[$cid]}; tok="${ATOKEN[$sym|$cid]}"
      # account0 spendable + gate payout liquidity of this token on this chain
      csend "$tok" "mint(address,uint256)" "$DEPLOYER_ADDR"  "$MINT" --rpc-url "${CRPC[$i]}" --private-key "$DEPLOYER_KEY"
      csend "$tok" "mint(address,uint256)" "${CGATE[$i]}"    "$MINT" --rpc-url "${CRPC[$i]}" --private-key "$DEPLOYER_KEY"
      # register this asset inbound from every OTHER chain it lives on
      for ocid in "${chs[@]}"; do
        [[ "$ocid" == "$cid" ]] && continue
        otok="${ATOKEN[$sym|$ocid]}"
        pad=$(printf '%064x' "$ocid"); did=$(cast keccak "0x${pad}${otok#0x}")
        csend "${CGATE[$i]}" "setLocalToken(bytes32,address)" "$did" "$tok" --rpc-url "${CRPC[$i]}" --private-key "$DEPLOYER_KEY"
      done
    done
  done
fi

# --- 4c. swap: same-chain SwapPool for the Swap view ---
if [[ "$ENABLE_SWAP" == "true" ]]; then
  if [[ "$DEPLOY_SWAP" == "true" ]]; then
    say "deploying SwapPool on ${CNAME[$swap_idx]}"
    if ( cd "$CONTRACTS" && forge script script/DeploySwap.s.sol:DeploySwap \
           --rpc-url "${CRPC[$swap_idx]}" --private-key "$DEPLOYER_KEY" --broadcast >"$RUN_DIR/swap-deploy.log" 2>&1 ); then
      source "$CONTRACTS/fixtures/swap-deploy.env"   # SWAP_POOL, STABLE, WETH, TT
      for pair in "$WETH:10000000000000000000" "$TT:250000000000000000000000" "$STABLE:5000000000"; do
        cast send "${pair%%:*}" "mint(address,uint256)" "$DEPLOYER_ADDR" "${pair##*:}" \
          --rpc-url "${CRPC[$swap_idx]}" --private-key "$DEPLOYER_KEY" >/dev/null 2>&1 || true
      done
      info "SwapPool=$SWAP_POOL (stable=$STABLE WETH=$WETH TT=$TT)"
    else
      info "!! swap deploy failed — Swap view will be empty (see $RUN_DIR/swap-deploy.log)"
      ENABLE_SWAP=false
    fi
  else
    SWAP_POOL="${SWAP_POOL_ADDR:-}"
    [[ "$SWAP_POOL" =~ ^0x ]] || { info "DEPLOY_SWAP=false and no SWAP_POOL_ADDR — Swap view off"; ENABLE_SWAP=false; }
  fi
fi
cd "$ROOT"

# Per-chain PRIMARY token = the first listed asset that exists on that chain.
# (The Gate bridges all assets; the demo UI surfaces this one per chain.)
CTOKEN=()
for i in "${!CID[@]}"; do
  primary=""
  for sym in "${ASYMS[@]}"; do
    t="${ATOKEN[$sym|${CID[$i]}]:-}"
    [[ -n "$t" ]] && { primary="$t"; break; }
  done
  CTOKEN[$i]="$primary"
done

# persist addresses for the summary / debugging
: > "$ADDR_ENV"
for i in "${!CID[@]}"; do echo "CHAIN_${CID[$i]}_GATE=${CGATE[$i]}" >> "$ADDR_ENV"; done
for sym in "${ASYMS[@]}"; do
  for cid in ${ACHAINS[$sym]}; do echo "TOKEN_${sym}_${cid}=${ATOKEN[$sym|$cid]}" >> "$ADDR_ENV"; done
done
echo "SWAP_POOL=${SWAP_POOL:-}" >> "$ADDR_ENV"

# reusable TOML fragments over all chains
emit_sources() {  # $1 = keyword: "sources" (validator) / "targets"/"sources" (keeper)
  for i in "${!CID[@]}"; do
    echo "[[$1]]"
    echo "chain_id = ${CID[$i]}"
    echo "rpc = \"${CRPC[$i]}\""
    echo "gate = \"${CGATE[$i]}\""
    echo "poll_interval_ms = 300"
    echo
  done
}

# ---------------------------------------------------------------------------
# 5. Postgres
# ---------------------------------------------------------------------------
if [[ "$PG_DOCKER" == "true" ]]; then
  say "starting Postgres ($PG_NAME on :$PG_PORT)"
  docker rm -f "$PG_NAME" >/dev/null 2>&1 || true
  docker run -d --name "$PG_NAME" \
    -e POSTGRES_USER=bridge -e POSTGRES_PASSWORD=bridge -e POSTGRES_DB=bridge \
    -p "${PG_PORT}:5432" postgres:16-alpine >/dev/null || die "failed to start Postgres container"
  ok=false
  for _ in $(seq 1 60); do docker exec "$PG_NAME" pg_isready -U bridge -d bridge >/dev/null 2>&1 && { ok=true; break; }; sleep 0.5; done
  $ok || die "Postgres did not become ready"
  info "Postgres ready"
fi

# ---------------------------------------------------------------------------
# 6. sig-store
# ---------------------------------------------------------------------------
say "starting sig-store ($STORE_URL)"
SIG_STORE_BIND="$BIND_HOST:$STORE_PORT" DATABASE_URL="$DATABASE_URL" \
  spawn "$ROOT/target/debug/sig-store" sig-store.log
ok=false
for _ in $(seq 1 60); do curl -s "$STORE_URL/health" | grep -q ok && { ok=true; break; }; sleep 0.25; done
$ok || die "sig-store did not come up (see $RUN_DIR/sig-store.log)"
info "sig-store healthy"

# ---------------------------------------------------------------------------
# 7. validators — watch ALL chains as sources; refund to ALL as destinations
# ---------------------------------------------------------------------------
say "starting ${#VALIDATOR_KEYS[@]} validator(s), each watching all $N chains"
vi=0
for key in "${VALIDATOR_KEYS[@]}"; do
  vi=$((vi+1))
  cfg="$RUN_DIR/validator-$vi.toml"
  {
    for i in "${!CID[@]}"; do
      echo "[[sources]]"
      echo "chain_id = ${CID[$i]}"
      echo "rpcs = [\"${CRPC[$i]}\"]"
      echo "gate = \"${CGATE[$i]}\""
      echo "start_block = 0"
      echo "block_confirmation = 0"
      echo "poll_interval_ms = 300"
      echo "max_block_range = 1000"
      echo "state_file = \"$RUN_DIR/validator-$vi-${CID[$i]}.json\""
      echo
    done
    echo "[signer]"
    echo "private_key = \"$key\""
    echo
    echo "[store]"
    echo "url = \"$STORE_URL\""
    if [[ "$ENABLE_REFUND" == "true" ]]; then
      echo
      echo "[refund]"
      echo "timeout_secs = $REFUND_TIMEOUT_SECS"
      echo "poll_interval_ms = 2000"
      echo "block_confirmation = $REFUND_BLOCK_CONFIRMATION"
      echo "allow_zero_confirmation = $REFUND_ALLOW_ZERO_CONFIRMATION"
      for i in "${!CID[@]}"; do
        echo
        echo "[[refund.destinations]]"
        echo "chain_id = ${CID[$i]}"
        echo "rpcs = [\"${CRPC[$i]}\"]"
        echo "gate = \"${CGATE[$i]}\""
      done
    fi
  } > "$cfg"
  spawn "$ROOT/target/debug/validator $cfg" "validator-$vi.log"
done

# ---------------------------------------------------------------------------
# 8. keeper — claims on ALL chains; refunds on ALL sources if enabled
# ---------------------------------------------------------------------------
say "starting keeper (targets = all $N chains)"
kcfg="$RUN_DIR/keeper.toml"
{
  emit_sources targets
  echo "[keeper]"
  echo "private_key = \"$KEEPER_KEY\""
  echo
  echo "[store]"
  echo "url = \"$STORE_URL\""
  if [[ "$ENABLE_REFUND" == "true" ]]; then
    echo
    emit_sources sources
  fi
} > "$kcfg"
spawn "$ROOT/target/debug/keeper $kcfg" keeper.log

# ---------------------------------------------------------------------------
# 9. indexer (history + refund eligibility) over ALL chains
# ---------------------------------------------------------------------------
if [[ "$ENABLE_INDEXER" == "true" ]]; then
  say "starting indexer"
  icfg="$RUN_DIR/indexer.toml"
  {
    echo "database_url = \"$DATABASE_URL\""
    echo "refund_timeout_secs = $REFUND_TIMEOUT_SECS"
    echo "sweep_interval_secs = $SWEEP_INTERVAL_SECS"
    for i in "${!CID[@]}"; do
      echo
      echo "[[chains]]"
      echo "chain_id = ${CID[$i]}"
      echo "rpc = \"${CRPC[$i]}\""
      echo "gate = \"${CGATE[$i]}\""
      [[ "$ENABLE_SWAP" == "true" && $i == "$swap_idx" && -n "${SWAP_POOL:-}" ]] && echo "pool = \"$SWAP_POOL\""
      echo "start_block = 0"
      echo "block_confirmation = 0"
      echo "poll_interval_ms = 500"
      echo "max_block_range = 1000"
    done
  } > "$icfg"
  spawn "$ROOT/target/debug/indexer $icfg" indexer.log
fi

# ---------------------------------------------------------------------------
# 10. graphql-api (backend) — registry + gates for ALL chains
# ---------------------------------------------------------------------------
say "starting graphql-api ($GQL_BIND)"
{
  echo "["
  for i in "${!CID[@]}"; do
    sep=","; [[ $i == $((N-1)) ]] && sep=""
    # per-chain token list from ASSETS (symbol + address), for the UI's picker
    toks=""
    for sym in "${ASYMS[@]}"; do
      t="${ATOKEN[$sym|${CID[$i]}]:-}"
      [[ -n "$t" ]] || continue
      [[ -n "$toks" ]] && toks+=", "
      toks+="{\"symbol\": \"$sym\", \"address\": \"$t\"}"
    done
    echo "  {\"chain_id\": ${CID[$i]}, \"name\": \"${CNAME[$i]}\", \"rpc_url\": \"${CRPC[$i]}\", \"gate\": \"${CGATE[$i]}\", \"token\": \"${CTOKEN[$i]}\", \"tokens\": [$toks]}$sep"
  done
  echo "]"
} > "$REG_JSON"

GQL_ARGS=(--bind "$GQL_BIND" --store-url "$STORE_URL" --threshold "$THRESHOLD"
          --chains-file "$REG_JSON" --allow-mutations)
for i in "${!CID[@]}"; do GQL_ARGS+=(--gate "${CID[$i]}=${CRPC[$i]},${CGATE[$i]}"); done
[[ "$ENABLE_INDEXER" == "true" ]] && GQL_ARGS+=(--db-url "$DATABASE_URL")
[[ "$ENABLE_SWAP" == "true" && -n "${SWAP_POOL:-}" ]] && GQL_ARGS+=(--swap "$SWAP_CHAIN=${CRPC[$swap_idx]},$SWAP_POOL")

spawn "$ROOT/target/debug/graphql-api ${GQL_ARGS[*]}" graphql-api.log
ok=false
for _ in $(seq 1 60); do curl -s "http://$GQL_BIND/health" >/dev/null 2>&1 && { ok=true; break; }; sleep 0.25; done
$ok || die "graphql-api did not come up (see $RUN_DIR/graphql-api.log)"
info "graphql-api healthy"

# ---------------------------------------------------------------------------
# 11. frontend (vite dev server)
# ---------------------------------------------------------------------------
say "starting frontend (vite on :$WEB_PORT)"
[[ -d "$FRONTEND/node_modules" ]] || ( cd "$FRONTEND" && npm install --no-audit --no-fund )
( cd "$FRONTEND" && VITE_PROXY_TARGET="http://$BIND_HOST:$GQL_PORT" \
    spawn "npx vite --host $WEB_HOST --port $WEB_PORT --strictPort" web.log )
for _ in $(seq 1 80); do curl -s "http://127.0.0.1:$WEB_PORT/" >/dev/null 2>&1 && break; sleep 0.3; done

# ---------------------------------------------------------------------------
# 12. summary
# ---------------------------------------------------------------------------
say "stack is up  ($N chains, ${#ASYMS[@]} asset(s), full-mesh bridging)"
printf '  %-12s %s\n' "frontend"  "http://$WEB_HOST:$WEB_PORT"
printf '  %-12s %s\n' "graphql"   "http://$GQL_BIND  (POST /graphql, GraphiQL at /)"
printf '  %-12s %s\n' "sig-store" "$STORE_URL"
for i in "${!CID[@]}"; do
  # list the assets bridgeable on this chain
  toks=""
  for sym in "${ASYMS[@]}"; do
    t="${ATOKEN[$sym|${CID[$i]}]:-}"; [[ -n "$t" ]] && toks+="$sym "
  done
  printf '  %-12s %s\n' "${CNAME[$i]}" "${CRPC[$i]}   gate ${CGATE[$i]}"
  printf '  %-12s   assets: %s\n' "" "${toks:-none}"
done
echo
echo "  MetaMask: add each network above, then import the deployer to move funds:"
echo "    address $DEPLOYER_ADDR"
echo "    key     $DEPLOYER_KEY"
echo
echo "  logs:  $RUN_DIR/*.log     addresses: $ADDR_ENV"
echo "  stop:  bash scripts/stop.sh $([ "$CONFIG" = "$ROOT/scripts/run.config" ] || echo "$CONFIG")"
