#!/usr/bin/env bash
# deploy-from-json.sh — deploy the bridge contracts from a JSON config.
#
#   bash scripts/deploy-from-json.sh [config.json] [--dry-run] [--no-config-update]
#
# Default config: config/deploy.config.json  (see config/README.md for the field
# reference). Two profiles:
#
#   "local"       forge-create path: Gate implementation + GateProxy, TestTokens
#                 for assets marked "auto", corridor registration + mint
#                 liquidity. The deployer stays the gate owner.
#   "production"  runs contracts/script/DeployProd.s.sol, which enforces >=3
#                 validators, a strict-majority threshold, a guardian, and hands
#                 ownership to a multisig (two-step). No tokens are deployed and
#                 NO corridor is registered here — after the handover only the
#                 owner may call setLocalToken, so the script emits the calldata
#                 for governance to execute instead.
#
# Writes every address it produced to `output.file`, and (unless
# --no-config-update) patches gate/token/pool addresses straight into the
# bridge runtime config named by `output.update_bridge_config`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS="$ROOT/contracts"
CONFIG="config/deploy.config.json"
DRY_RUN=false
UPDATE_CFG=true

for arg in "$@"; do
  case "$arg" in
    --dry-run)          DRY_RUN=true ;;
    --no-config-update) UPDATE_CFG=false ;;
    -h|--help)          sed -n '2,25p' "$0"; exit 0 ;;
    -*)                 echo "unknown flag: $arg" >&2; exit 1 ;;
    *)                  CONFIG="$arg" ;;
  esac
done
[[ "$CONFIG" = /* ]] || CONFIG="$ROOT/$CONFIG"
[[ -f "$CONFIG" ]] || { echo "config not found: $CONFIG" >&2; exit 1; }

say()  { printf '\n\033[1;36m=== %s ===\033[0m\n' "$*"; }
info() { printf '  %s\n' "$*"; }
warn() { printf '\033[1;33m  ! %s\033[0m\n' "$*"; }
die()  { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "'$1' not found on PATH (needed for: $2)"; }

need jq    "reading the JSON config"
need forge "contract deploys"
need cast  "chain reads/writes"

j()  { jq -r "$1" "$CONFIG"; }
jr() { jq -r "$1 // empty" "$CONFIG"; }

# --- config ----------------------------------------------------------------
NAME="$(j '.name')"
PROFILE="$(j '.profile')"
[[ "$PROFILE" == "local" || "$PROFILE" == "production" ]] || die "profile must be \"local\" or \"production\" (got: $PROFILE)"
THRESHOLD="$(j '.gate.threshold')"
mapfile -t VALIDATORS < <(j '.gate.validators[]')
GUARDIAN="$(jr '.gate.guardian')"
OWNER="$(jr '.gate.owner')"
BRIDGE_DOMAIN="$(j '.gate.bridge_domain')"
OUT_FILE="$(jr '.output.file')"; OUT_FILE="${OUT_FILE:-config/deployments/$NAME.json}"
[[ "$OUT_FILE" = /* ]] || OUT_FILE="$ROOT/$OUT_FILE"
BRIDGE_CFG="$(jr '.output.update_bridge_config')"
[[ -n "$BRIDGE_CFG" && "$BRIDGE_CFG" != /* ]] && BRIDGE_CFG="$ROOT/$BRIDGE_CFG"

(( ${#VALIDATORS[@]} >= 1 )) || die "gate.validators is empty"
(( THRESHOLD >= 1 && THRESHOLD <= ${#VALIDATORS[@]} )) || die "gate.threshold must be 1..${#VALIDATORS[@]}"
for v in "${VALIDATORS[@]}"; do [[ "$v" =~ ^0x[0-9a-fA-F]{40}$ ]] || die "not an address: gate.validators[] = $v"; done
# Duplicates would silently shrink the quorum: the Gate constructor dedupes, so
# [A,B,B] with threshold 2 ships a 2-of-2 gate, one key short of what was signed off.
dupes="$(printf '%s\n' "${VALIDATORS[@]}" | tr 'A-F' 'a-f' | sort | uniq -d)"
[[ -z "$dupes" ]] || die "duplicate validator address: $dupes"

# --- profile policy --------------------------------------------------------
if [[ "$PROFILE" == "production" ]]; then
  (( ${#VALIDATORS[@]} >= 3 )) || die "production needs >= 3 validators (DeployProd rejects fewer)"
  (( THRESHOLD >= 2 && THRESHOLD * 2 > ${#VALIDATORS[@]} )) || die "production needs a strict-majority threshold (> ${#VALIDATORS[@]}/2, and >= 2)"
  [[ "$GUARDIAN" =~ ^0x[0-9a-fA-F]{40}$ ]] || die "production needs gate.guardian"
  [[ "$OWNER"    =~ ^0x[0-9a-fA-F]{40}$ ]] || die "production needs gate.owner (the multisig)"
  [[ "${GUARDIAN,,}" != "${OWNER,,}" ]] || die "gate.guardian must differ from gate.owner"
  [[ "$BRIDGE_DOMAIN" == "auto" ]] && die "production must PIN gate.bridge_domain (0x + 64 hex) — 'auto' would mint a new one on every run"
  [[ "$(jq -r '[.assets[]?.deployments[]? | select(.address == "auto")] | length' "$CONFIG")" == "0" ]] \
    || die "production cannot deploy TestTokens: replace every asset address \"auto\" with the real ERC-20"
  [[ "$(jq -r '[.assets[]? | select(.test_liquidity.enabled == true)] | length' "$CONFIG")" == "0" ]] \
    || die "production cannot mint test liquidity: set assets[].test_liquidity.enabled = false"
fi

# Every gate in one mesh generation shares ONE domain, and a NEW generation needs
# a NEW one — that is what stops the previous deployment's validator signatures
# from replaying against these fresh gates.
if [[ "$BRIDGE_DOMAIN" == "auto" ]]; then
  BRIDGE_DOMAIN="$(cast keccak "$(printf 'selendra-bridge|%s|%s|%s|%s' \
    "$NAME" "$(IFS=,; echo "${VALIDATORS[*]}")" "$THRESHOLD" "$(date +%s)-$$")")"
  info "generated bridge_domain=$BRIDGE_DOMAIN (pin it in the config to reuse)"
fi
[[ "$BRIDGE_DOMAIN" =~ ^0x[0-9a-fA-F]{64}$ ]] || die "gate.bridge_domain must be 0x + 64 hex chars"
[[ "$BRIDGE_DOMAIN" =~ ^0x0{64}$ ]] && die "gate.bridge_domain must not be zero (Gate rejects it)"

# --- deployer auth (private key / env var / encrypted keystore) -------------
AUTH=()
DEPLOYER_KEY="$(jr '.deployer.private_key')"
DEPLOYER_KEY_ENV="$(jr '.deployer.private_key_env')"
KEYSTORE="$(jr '.deployer.keystore')"
KEYSTORE_PASS_FILE="$(jr '.deployer.keystore_password_file')"
if [[ -n "$KEYSTORE" ]]; then
  [[ -f "$KEYSTORE" ]] || die "deployer.keystore not found: $KEYSTORE"
  AUTH=(--keystore "$KEYSTORE")
  [[ -n "$KEYSTORE_PASS_FILE" ]] && AUTH+=(--password-file "$KEYSTORE_PASS_FILE")
  DEPLOYER_ADDR="$(cast wallet address --keystore "$KEYSTORE" ${KEYSTORE_PASS_FILE:+--password-file "$KEYSTORE_PASS_FILE"})"
elif [[ -n "$DEPLOYER_KEY_ENV" ]]; then
  key="${!DEPLOYER_KEY_ENV:-}"
  [[ -n "$key" ]] || die "deployer.private_key_env=$DEPLOYER_KEY_ENV is set in the config but that env var is empty"
  AUTH=(--private-key "$key")
  DEPLOYER_ADDR="$(cast wallet address --private-key "$key")"
elif [[ -n "$DEPLOYER_KEY" ]]; then
  [[ "$PROFILE" == "local" ]] && : || warn "profile=production with an INLINE deployer key — prefer deployer.keystore"
  AUTH=(--private-key "$DEPLOYER_KEY")
  DEPLOYER_ADDR="$(cast wallet address --private-key "$DEPLOYER_KEY")"
else
  die "no deployer key: set deployer.keystore (preferred), deployer.private_key_env, or deployer.private_key"
fi

say "deploy plan: $NAME (profile=$PROFILE)"
info "deployer : $DEPLOYER_ADDR"
info "gate     : ${#VALIDATORS[@]} validators, threshold $THRESHOLD"
info "domain   : $BRIDGE_DOMAIN"
info "chains   : $(j '[.chains[].chain_id] | join(", ")')"
info "assets   : $(j '[.assets[]?.symbol] | join(", ") | if . == "" then "none" else . end')"
$DRY_RUN && { echo; info "--dry-run: nothing was sent"; exit 0; }

# --- helpers ---------------------------------------------------------------
deployed_to() { grep deployedTo | grep -oE '0x[0-9a-fA-F]{40}' | head -1; }
fc()    { ( cd "$CONTRACTS" && forge create "$1" --rpc-url "$2" "${AUTH[@]}" --broadcast --json "${@:3}" ) | deployed_to; }
csend() { cast send "$1" "$2" "${@:3}" "${AUTH[@]}" >/dev/null; }
debridge_id() { printf '0x%064x%s\n' "$1" "${2#0x}" | xargs cast keccak; }   # keccak(packed(uint256,address))
scaled()      { local whole="$1" dec="$2"; [[ "$whole" =~ ^[0-9]+$ ]] || die "amount must be a whole number: $whole"
                printf '%s%s\n' "$whole" "$(printf '0%.0s' $(seq 1 "$dec"))"; }

RUN_LOG_DIR="$(dirname "$OUT_FILE")/logs"
mkdir -p "$RUN_LOG_DIR"

say "building contracts"
( cd "$CONTRACTS" && forge build >/dev/null ) || die "forge build failed"

# --- chains: verify RPC, record floor block, deploy gates -------------------
declare -A GATE IMPL FLOOR RPC CNAME
mapfile -t CHAIN_IDS < <(j '.chains[].chain_id')
for cid in "${CHAIN_IDS[@]}"; do
  RPC[$cid]="$(j ".chains[] | select(.chain_id == $cid) | .rpc_url")"
  CNAME[$cid]="$(j ".chains[] | select(.chain_id == $cid) | .name")"
  got="$(cast chain-id --rpc-url "${RPC[$cid]}" 2>/dev/null)" || die "RPC unreachable: ${RPC[$cid]}"
  [[ "$got" == "$cid" ]] || die "${RPC[$cid]} reports chainId $got, config says $cid"
  FLOOR[$cid]="$(cast block-number --rpc-url "${RPC[$cid]}")"
done

say "deploying gates"
for cid in "${CHAIN_IDS[@]}"; do
  deploy_gate="$(j ".chains[] | select(.chain_id == $cid) | .deploy_gate")"
  existing="$(jq -r ".chains[] | select(.chain_id == $cid) | .gate // empty" "$CONFIG")"
  if [[ "$deploy_gate" != "true" ]]; then
    [[ "$existing" =~ ^0x[0-9a-fA-F]{40}$ ]] || die "chain $cid has deploy_gate=false but no gate address"
    GATE[$cid]="$existing"; info "${CNAME[$cid]} ($cid) reusing gate ${GATE[$cid]}"; continue
  fi

  if [[ "$PROFILE" == "production" ]]; then
    # DeployProd asserts every policy invariant on-chain and reverts the whole
    # deployment if one is off; it also appoints the guardian and starts the
    # ownership handover to the multisig.
    log="$RUN_LOG_DIR/deploy-prod-$cid.log"
    # Subshell + "${AUTH[@]}" rather than `bash -c "... ${AUTH[*]}"`: the array
    # form keeps a keystore path with spaces intact and never re-splits the key.
    ( cd "$CONTRACTS" \
      && EXPECTED_CHAIN_ID="$cid" \
         VALIDATORS="$(IFS=,; echo "${VALIDATORS[*]}")" \
         THRESHOLD="$THRESHOLD" GUARDIAN="$GUARDIAN" OWNER="$OWNER" BRIDGE_DOMAIN="$BRIDGE_DOMAIN" \
         forge script script/DeployProd.s.sol:DeployProd \
           --rpc-url "${RPC[$cid]}" "${AUTH[@]}" --broadcast ) >"$log" 2>&1 \
      || { tail -30 "$log"; die "DeployProd failed on chain $cid (log: $log)"; }
    GATE[$cid]="$(grep -A0 'Gate deployed' "$log" | grep -oE '0x[0-9a-fA-F]{40}' | head -1)"
    [[ "${GATE[$cid]}" =~ ^0x ]] || { cat "$log"; die "could not read the deployed gate address on chain $cid"; }
    info "${CNAME[$cid]} ($cid) gate=${GATE[$cid]}  (guardian set, ownership pending $OWNER)"
  else
    # UUPS: an implementation (never initialized) + the proxy that IS the gate.
    # initialize runs inside the proxy constructor, so no uninitialized proxy
    # ever exists on-chain for someone else to claim.
    initdata="$(cast calldata 'initialize(address[],uint256,bytes32)' "[$(IFS=,; echo "${VALIDATORS[*]}")]" "$THRESHOLD" "$BRIDGE_DOMAIN")"
    IMPL[$cid]="$(fc src/Gate.sol:Gate "${RPC[$cid]}")"
    [[ "${IMPL[$cid]}" =~ ^0x ]] || die "gate implementation deploy failed on chain $cid"
    GATE[$cid]="$(fc src/GateProxy.sol:GateProxy "${RPC[$cid]}" --constructor-args "${IMPL[$cid]}" "$initdata")"
    [[ "${GATE[$cid]}" =~ ^0x ]] || die "gate proxy deploy failed on chain $cid"
    info "${CNAME[$cid]} ($cid) gate=${GATE[$cid]} (implementation ${IMPL[$cid]})"
  fi
done

# --- assets: resolve/deploy tokens -----------------------------------------
declare -A TOKEN          # "SYM|chain_id" -> address
declare -A ASSET_CHAINS   # "SYM" -> "cid cid"
mapfile -t SYMS < <(j '.assets[]?.symbol')
if (( ${#SYMS[@]} )); then
  say "resolving asset tokens"
  for sym in "${SYMS[@]}"; do
    tname="$(j ".assets[] | select(.symbol == \"$sym\") | .name")"
    for cid in $(j ".assets[] | select(.symbol == \"$sym\") | .deployments[].chain_id"); do
      [[ -n "${RPC[$cid]:-}" ]] || die "asset $sym lists chain $cid, which is not in .chains"
      addr="$(j ".assets[] | select(.symbol == \"$sym\") | .deployments[] | select(.chain_id == $cid) | .address")"
      if [[ "$addr" == "auto" ]]; then
        addr="$(fc src/TestToken.sol:TestToken "${RPC[$cid]}" --constructor-args "$tname" "$sym")"
        [[ "$addr" =~ ^0x ]] || die "TestToken deploy failed: $sym on chain $cid"
      else
        [[ "$addr" =~ ^0x[0-9a-fA-F]{40}$ ]] || die "asset $sym on chain $cid: '$addr' is neither \"auto\" nor an address"
        code="$(cast code "$addr" --rpc-url "${RPC[$cid]}" 2>/dev/null || echo 0x)"
        [[ "$code" != "0x" && -n "$code" ]] || die "asset $sym on chain $cid: no contract code at $addr"
      fi
      TOKEN["$sym|$cid"]="$addr"
      ASSET_CHAINS[$sym]="${ASSET_CHAINS[$sym]:-} $cid"
      info "$sym @ $cid = $addr"
    done
  done
fi

# --- corridors: setLocalToken (write-once, owner-only) ----------------------
CORRIDOR_CALLS='[]'
for sym in "${SYMS[@]:-}"; do
  [[ -z "${sym:-}" ]] && continue
  [[ "$(j ".assets[] | select(.symbol == \"$sym\") | .register_corridors")" == "true" ]] || continue
  read -ra chs <<<"${ASSET_CHAINS[$sym]}"
  (( ${#chs[@]} >= 2 )) || { warn "$sym lives on <2 chains — nothing to bridge"; continue; }
  say "registering $sym corridors (${#chs[@]} chains, full mesh)"
  for cid in "${chs[@]}"; do
    for ocid in "${chs[@]}"; do
      [[ "$ocid" == "$cid" ]] && continue
      did="$(debridge_id "$ocid" "${TOKEN[$sym|$ocid]}")"
      local_tok="${TOKEN[$sym|$cid]}"
      if [[ "$PROFILE" == "production" ]]; then
        # Ownership is (pending) with the multisig, so only governance can do this.
        data="$(cast calldata 'setLocalToken(bytes32,address)' "$did" "$local_tok")"
        CORRIDOR_CALLS="$(jq -c --arg c "$cid" --arg to "${GATE[$cid]}" --arg d "$data" \
          --arg sym "$sym" --arg from "$ocid" \
          '. + [{chain_id: ($c|tonumber), to: $to, data: $d, note: ("register \($sym) inbound from chain \($from)")}]' <<<"$CORRIDOR_CALLS")"
        continue
      fi
      cur="$(cast call "${GATE[$cid]}" 'tokenOf(bytes32)(address)' "$did" --rpc-url "${RPC[$cid]}" 2>/dev/null || echo "")"
      if [[ -z "$cur" || "$cur" =~ ^0x0{40}$ ]]; then
        csend "${GATE[$cid]}" 'setLocalToken(bytes32,address)' "$did" "$local_tok" --rpc-url "${RPC[$cid]}"
        info "chain $cid <- $sym from chain $ocid  ($did)"
      else
        # setLocalToken is WRITE-ONCE: in-flight claims bind only the debridgeId,
        # so repointing a live corridor would release the wrong asset.
        info "chain $cid <- $sym from chain $ocid already registered ($cur)"
      fi
    done
  done
done
if [[ "$PROFILE" == "production" && "$CORRIDOR_CALLS" != "[]" ]]; then
  warn "corridors NOT registered: the gate owner is the multisig. Execute output.governance_calls after acceptOwnership()."
fi

# --- test liquidity (local only) -------------------------------------------
for sym in "${SYMS[@]:-}"; do
  [[ -z "${sym:-}" ]] && continue
  [[ "$(j ".assets[] | select(.symbol == \"$sym\") | .test_liquidity.enabled")" == "true" ]] || continue
  dec="$(j ".assets[] | select(.symbol == \"$sym\") | .decimals")"
  to_dep="$(j ".assets[] | select(.symbol == \"$sym\") | .test_liquidity.mint_to_deployer")"
  to_gate="$(j ".assets[] | select(.symbol == \"$sym\") | .test_liquidity.mint_to_gate")"
  say "minting $sym test liquidity"
  for cid in ${ASSET_CHAINS[$sym]}; do
    tok="${TOKEN[$sym|$cid]}"
    [[ "$to_dep"  == "0" ]] || csend "$tok" 'mint(address,uint256)' "$DEPLOYER_ADDR" "$(scaled "$to_dep" "$dec")"  --rpc-url "${RPC[$cid]}"
    [[ "$to_gate" == "0" ]] || csend "$tok" 'mint(address,uint256)' "${GATE[$cid]}"  "$(scaled "$to_gate" "$dec")" --rpc-url "${RPC[$cid]}"
    info "chain $cid: $to_dep to deployer, $to_gate to gate"
  done
done

# --- optional same-chain SwapPool ------------------------------------------
SWAP_POOL="$(jr '.swap.pool')"; SWAP_CHAIN="$(jr '.swap.chain_id')"; SWAP_JSON='null'
if [[ "$(j '.swap.enabled')" == "true" ]]; then
  [[ -n "${RPC[$SWAP_CHAIN]:-}" ]] || die "swap.chain_id=$SWAP_CHAIN is not in .chains"
  if [[ "$(j '.swap.deploy')" == "true" ]]; then
    [[ "$PROFILE" == "local" ]] || die "swap.deploy=true is local-only (DeploySwap mints unrestricted test tokens)"
    say "deploying SwapPool on chain $SWAP_CHAIN"
    ( cd "$CONTRACTS" && forge script script/DeploySwap.s.sol:DeploySwap --rpc-url "${RPC[$SWAP_CHAIN]}" "${AUTH[@]}" --broadcast >/dev/null ) \
      || die "DeploySwap failed"
    # shellcheck disable=SC1091
    source "$CONTRACTS/fixtures/swap-deploy.env"   # SWAP_POOL, STABLE, WETH, TT
    info "pool=$SWAP_POOL stable=$STABLE weth=$WETH tt=$TT"
    # The pool's token list is discovered by replaying its TokenListed logs, so a
    # scan floor AFTER those listings reports a pool with zero tokens — and a
    # floor of 0 is worse: on a live chain it is a genesis-to-tip filter that
    # hosted RPCs reject outright. Pin it to the height captured before the
    # deploy.
    SWAP_JSON="$(jq -n --argjson c "$SWAP_CHAIN" --arg p "$SWAP_POOL" --arg s "$STABLE" --arg w "$WETH" --arg t "$TT" \
      --argjson fb "${FLOOR[$SWAP_CHAIN]}" '{chain_id:$c, pool:$p, from_block:$fb, stable:$s, weth:$w, tt:$t}')"
  else
    [[ "$SWAP_POOL" =~ ^0x[0-9a-fA-F]{40}$ ]] || die "swap.enabled with deploy=false needs swap.pool"
    # Reusing a pool: its listings predate this run, so keep whatever floor the
    # runtime config already carries rather than inventing one.
    SWAP_JSON="$(jq -n --argjson c "$SWAP_CHAIN" --arg p "$SWAP_POOL" '{chain_id:$c, pool:$p, from_block:null}')"
  fi
fi

# --- Solana leg: program, gate config, corridors, assets --------------------
#
# A different VM, a different toolchain and a different process. What ties it to
# the EVM gates is exactly two values: the SAME validator set and the SAME
# bridge_domain — the domain is hashed into every submissionId on both VMs, so a
# mismatch means no id ever agrees and nothing bridges (loudly, not silently).
SOLANA_JSON='null'
if [[ "$(j '.solana.enabled // false')" == "true" ]]; then
  SOL_CHAIN_ID="$(j '.solana.chain_id')"
  SOL_RPC="$(j '.solana.rpc')"
  for cid in "${CHAIN_IDS[@]}"; do
    [[ "$cid" == "$SOL_CHAIN_ID" ]] && die "solana.chain_id $SOL_CHAIN_ID collides with an EVM chain in .chains"
  done

  # The solana CLI is not usually on PATH; fall back to the standard install dir.
  command -v solana >/dev/null 2>&1 || export PATH="$HOME/.local/share/solana/install/active_release/bin:$PATH"
  need solana "the Solana program deploy"

  GATE_ADMIN="$(jr '.solana.gate_admin_bin')"; GATE_ADMIN="${GATE_ADMIN:-crates/solana-relayer/target/debug/gate-admin}"
  [[ "$GATE_ADMIN" = /* ]] || GATE_ADMIN="$ROOT/$GATE_ADMIN"
  if [[ ! -x "$GATE_ADMIN" ]]; then
    [[ "$(j '.solana.build')" == "true" ]] || die "no gate-admin at $GATE_ADMIN (set solana.build = true, or point solana.gate_admin_bin at your build)"
    # Its own cargo project: solana-client pins zeroize <1.4, alloy needs ^1.5,
    # so no EVM-side crate can host this tool.
    ( cd "$ROOT" && cargo build --manifest-path crates/solana-relayer/Cargo.toml --bin gate-admin ) || die "building gate-admin failed"
  fi

  PAYER="$(j '.solana.payer_keypair')"; [[ "$PAYER" = /* ]] || PAYER="$ROOT/$PAYER"
  [[ -f "$PAYER" ]] || die "solana.payer_keypair not found: $PAYER"
  PAYER_PUBKEY="$(solana address --keypair "$PAYER")"

  say "solana leg ($(j '.solana.cluster'), chain id $SOL_CHAIN_ID)"
  info "payer   : $PAYER_PUBKEY  ($(solana balance --keypair "$PAYER" --url "$SOL_RPC" 2>/dev/null || echo 'balance unknown'))"

  # --- program ---
  if [[ "$(j '.solana.program.deploy')" == "true" ]]; then
    so="$(j '.solana.program.so_path')"; [[ "$so" = /* ]] || so="$ROOT/$so"
    [[ -f "$so" ]] || die "program binary not found: $so (build it with scripts/testing/build-solana.sh)"
    out="$RUN_LOG_DIR/solana-deploy.json"
    # --use-rpc sends the write transactions over JSON-RPC instead of straight to
    # the leader's TPU. The TPU path needs gossip reachability, which a hosted
    # endpoint or a containerised validator does not give you — it fails with
    # "Failed find any cluster node info for upcoming leaders" after a 20s stall.
    rpc_flag=(); [[ "$(j '.solana.program.use_rpc')" != "false" ]] && rpc_flag=(--use-rpc)
    solana program deploy "$so" --url "$SOL_RPC" --keypair "$PAYER" "${rpc_flag[@]}" --output json > "$out" \
      || { cat "$out"; die "solana program deploy failed"; }
    SOL_PROGRAM="$(jq -r '.programId' "$out")"
    info "program : $SOL_PROGRAM (deployed)"
  else
    SOL_PROGRAM="$(jr '.solana.program.program_id')"
    [[ -n "$SOL_PROGRAM" ]] || die "solana.program.deploy = false needs solana.program.program_id"
    info "program : $SOL_PROGRAM (existing)"
  fi

  ga() { "$GATE_ADMIN" --rpc "$SOL_RPC" --keypair "$PAYER" --program "$SOL_PROGRAM" "$@"; }
  # A freshly deployed program is not instantly visible at the commitment the
  # admin client reads at: the first instruction after a deploy fails with
  # "invalid account data" while the ProgramData account is still settling.
  # Retry rather than make every operator hit that once and guess.
  ga_retry() {
    local out attempt
    for attempt in 1 2 3 4 5; do
      if out="$(ga "$@" 2>&1)"; then return 0; fi
      sleep 3
    done
    echo "$out" >&2
    return 1
  }

  # Wait for the program account itself before touching it at all.
  for _ in $(seq 1 20); do
    solana program show "$SOL_PROGRAM" --url "$SOL_RPC" >/dev/null 2>&1 && break
    sleep 2
  done

  # --- init (idempotent: the program refuses a second init, so read first) ---
  show="$(ga show 2>&1)" || { echo "$show"; die "gate-admin show failed"; }
  if grep -q "NOT INITIALIZED" <<<"$show"; then
    [[ "$(j '.solana.init.run')" == "true" ]] || die "the gate program is not initialized and solana.init.run = false"
    vargs=(); for v in "${VALIDATORS[@]}"; do vargs+=(--validator "$v"); done
    guardian="$(jr '.solana.init.guardian')"
    # `init` must be signed by the program's UPGRADE AUTHORITY, and the signer
    # becomes the gate owner. There is no ownership-transfer instruction on this
    # side — unlike the EVM gate's two-step handover — so whichever key deploys
    # the program is the key that governs it. Guard it accordingly.
    ga_retry init --chain-id "$SOL_CHAIN_ID" --threshold "$THRESHOLD" "${vargs[@]}" \
       --bridge-domain "$BRIDGE_DOMAIN" \
       --max-validators "$(j '.solana.init.max_validators')" \
       --max-corridors "$(j '.solana.init.max_corridors')" \
       ${guardian:+--guardian "$guardian"} >/dev/null || die "gate-admin init failed"
    info "init    : $THRESHOLD-of-${#VALIDATORS[@]}, owner $PAYER_PUBKEY${guardian:+, guardian $guardian}"
    show="$(ga show 2>&1)"
  else
    info "init    : already initialized (leaving it alone)"
  fi

  # An existing program from an EARLIER generation is the failure this catches:
  # its domain is baked in at init and can never be mutated, so it would sign
  # ids the EVM gates of this mesh reject — and the only symptom is transfers
  # that never claim.
  on_chain_domain="$(grep -oE 'bridge domain: 0x[0-9a-fA-F]{64}' <<<"$show" | grep -oE '0x[0-9a-fA-F]{64}' || true)"
  if [[ -n "$on_chain_domain" && "${on_chain_domain,,}" != "${BRIDGE_DOMAIN,,}" ]]; then
    die "the Solana gate's bridge_domain is $on_chain_domain but this deployment's is $BRIDGE_DOMAIN — that program belongs to a different generation and no submissionId would ever agree. Deploy a fresh program, or pin gate.bridge_domain to the on-chain value."
  fi

  # --- corridors: every EVM chain this mesh can send to ---
  SOL_CORRIDORS='[]'
  if [[ "$(j '.solana.register_corridors')" == "true" ]]; then
    for cid in "${CHAIN_IDS[@]}"; do
      # `send` refuses any chain_id_to that governance has not registered here
      # (H-3), and the instruction is idempotent, so re-running is free.
      ga_retry register-corridor --chain-id-to "$cid" >/dev/null || die "register-corridor $cid failed"
      SOL_CORRIDORS="$(jq -c --argjson c "$cid" '. + [$c]' <<<"$SOL_CORRIDORS")"
      info "corridor: -> chain $cid"
    done
  fi

  # --- assets: bind each corridor's debridgeId to the SPL mint + vault ---
  #
  # One registration per SOURCE chain, exactly as the EVM side needs one
  # setLocalToken per corridor: a claim commits only to the debridgeId, and the
  # id differs per origin chain. The mint and vault are supplied, never created
  # here — the vault must already be an SPL account owned by the program's
  # vault_authority PDA with no delegate or close authority.
  SOL_ASSETS='[]'
  for sym in $(j '.solana.assets[]?.symbol'); do
    mint="$(jr ".solana.assets[] | select(.symbol == \"$sym\") | .mint")"
    vault="$(jr ".solana.assets[] | select(.symbol == \"$sym\") | .vault")"
    [[ -n "$mint" && -n "$vault" ]] || { warn "solana asset $sym has no mint/vault — skipped (create them first, then re-run)"; continue; }
    from="$(jq -c ".solana.assets[] | select(.symbol == \"$sym\") | .from_chains" "$CONFIG")"
    ids='[]'
    for cid in ${ASSET_CHAINS[$sym]:-}; do
      if [[ "$from" != '"all"' ]]; then
        jq -e --argjson c "$cid" 'index($c) != null' <<<"$from" >/dev/null || continue
      fi
      did="$(debridge_id "$cid" "${TOKEN[$sym|$cid]}")"
      ga_retry register-asset --debridge-id "$did" --mint "$mint" --vault "$vault" >/dev/null \
        || die "register-asset $sym (from chain $cid) failed"
      info "asset   : $sym from chain $cid -> mint $mint"
      ids="$(jq -c --arg d "$did" --argjson c "$cid" '. + [{from_chain: $c, debridge_id: $d}]' <<<"$ids")"
    done
    # A Solana-NATIVE asset has no EVM-derived id, so the operator names one; it
    # is then registered on the EVM gates the same way any inbound corridor is.
    native_did="$(jr ".solana.assets[] | select(.symbol == \"$sym\") | .debridge_id")"
    if [[ -n "$native_did" ]]; then
      ga_retry register-asset --debridge-id "$native_did" --mint "$mint" --vault "$vault" >/dev/null \
        || die "register-asset $sym (solana-native id) failed"
      for cid in ${ASSET_CHAINS[$sym]:-}; do
        local_tok="${TOKEN[$sym|$cid]}"
        if [[ "$PROFILE" == "production" ]]; then
          data="$(cast calldata 'setLocalToken(bytes32,address)' "$native_did" "$local_tok")"
          CORRIDOR_CALLS="$(jq -c --argjson c "$cid" --arg to "${GATE[$cid]}" --arg d "$data" --arg sym "$sym" \
            '. + [{chain_id: $c, to: $to, data: $d, note: ("register \($sym) inbound from Solana")}]' <<<"$CORRIDOR_CALLS")"
        else
          cur="$(cast call "${GATE[$cid]}" 'tokenOf(bytes32)(address)' "$native_did" --rpc-url "${RPC[$cid]}" 2>/dev/null || echo "")"
          if [[ -z "$cur" || "$cur" =~ ^0x0{40}$ ]]; then
            csend "${GATE[$cid]}" 'setLocalToken(bytes32,address)' "$native_did" "$local_tok" --rpc-url "${RPC[$cid]}"
            info "chain $cid <- $sym from Solana ($native_did)"
          fi
        fi
      done
      ids="$(jq -c --arg d "$native_did" '. + [{from_chain: "solana", debridge_id: $d}]' <<<"$ids")"
    fi
    SOL_ASSETS="$(jq -c --arg s "$sym" --arg m "$mint" --arg v "$vault" --argjson ids "$ids" \
      '. + [{symbol: $s, mint: $m, vault: $v, registrations: $ids}]' <<<"$SOL_ASSETS")"
  done

  SOLANA_JSON="$(jq -n --argjson cid "$SOL_CHAIN_ID" --arg rpc "$SOL_RPC" --arg prog "$SOL_PROGRAM" \
    --arg owner "$PAYER_PUBKEY" --argjson cor "$SOL_CORRIDORS" --argjson assets "$SOL_ASSETS" \
    '{chain_id:$cid, rpc:$rpc, program_id:$prog, owner:$owner, corridors:$cor, assets:$assets}')"
fi

# --- record ----------------------------------------------------------------
say "writing $OUT_FILE"
mkdir -p "$(dirname "$OUT_FILE")"
chains_json='[]'
for cid in "${CHAIN_IDS[@]}"; do
  toks='{}'
  for sym in "${SYMS[@]:-}"; do
    [[ -n "${TOKEN[$sym|$cid]:-}" ]] || continue
    toks="$(jq -c --arg s "$sym" --arg a "${TOKEN[$sym|$cid]}" '. + {($s): $a}' <<<"$toks")"
  done
  chains_json="$(jq -c --argjson cid "$cid" --arg name "${CNAME[$cid]}" --arg rpc "${RPC[$cid]}" \
    --arg gate "${GATE[$cid]}" --arg impl "${IMPL[$cid]:-}" --argjson floor "${FLOOR[$cid]}" --argjson toks "$toks" \
    '. + [{chain_id:$cid, name:$name, rpc_url:$rpc, gate:$gate, gate_implementation:(if $impl == "" then null else $impl end), deploy_block:$floor, tokens:$toks}]' \
    <<<"$chains_json")"
done
jq -n --arg name "$NAME" --arg profile "$PROFILE" --arg domain "$BRIDGE_DOMAIN" \
      --arg deployer "$DEPLOYER_ADDR" --arg at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      --argjson vals "$(printf '%s\n' "${VALIDATORS[@]}" | jq -R . | jq -s .)" \
      --argjson th "$THRESHOLD" --argjson chains "$chains_json" --argjson swap "$SWAP_JSON" \
      --argjson gov "$CORRIDOR_CALLS" --argjson solana "$SOLANA_JSON" \
  '{name:$name, profile:$profile, deployed_at:$at, deployer:$deployer, bridge_domain:$domain,
    validators:$vals, threshold:$th, chains:$chains, swap:$swap, solana:$solana,
    governance_calls:$gov}' > "$OUT_FILE"

# --- patch the runtime config ----------------------------------------------
if $UPDATE_CFG && [[ -n "$BRIDGE_CFG" ]]; then
  [[ -f "$BRIDGE_CFG" ]] || die "output.update_bridge_config points at a missing file: $BRIDGE_CFG"
  say "updating $BRIDGE_CFG"
  tmp="$(mktemp)"
  jq --slurpfile d "$OUT_FILE" '
    ($d[0]) as $dep
    | .threshold = $dep.threshold
    | .chains = [ .chains[] as $c
        | ($dep.chains[] | select(.chain_id == $c.chain_id)) as $x
        | if $x == null then $c else
            $c + { gate: $x.gate,
                   start_block: $x.deploy_block,
                   tokens: ($x.tokens | to_entries | map({symbol: .key, address: .value})) }
          end ]
    | if $dep.solana != null
      then .solana = ((.solana // {}) + { enabled: true, chain_id: $dep.solana.chain_id,
                                          rpc: $dep.solana.rpc, program_id: $dep.solana.program_id })
         | .solana.tokens = [ $dep.solana.assets[] | {symbol, mint} ]
      else . end
    | if $dep.swap != null
      then .graphql.swap = { enabled: true, chain_id: $dep.swap.chain_id, pool: $dep.swap.pool,
                             from_block: ($dep.swap.from_block // .graphql.swap.from_block // 0) }
         | .chains = [ .chains[] | if .chain_id == $dep.swap.chain_id then .pool = $dep.swap.pool else . end ]
      else . end
  ' "$BRIDGE_CFG" > "$tmp" && mv "$tmp" "$BRIDGE_CFG"
  info "gate + token + start_block addresses written into the runtime config"
fi

say "done"
info "addresses : $OUT_FILE"
info "domain    : $BRIDGE_DOMAIN  (every gate in this mesh generation shares it)"
[[ "$PROFILE" == "production" ]] && {
  info "next      : the multisig $OWNER must call acceptOwnership() on every gate,"
  info "            then execute .governance_calls from $OUT_FILE to register corridors"
}
info "run       : bash scripts/bridge-from-json.sh ${BRIDGE_CFG:-config/bridge.config.json}"
