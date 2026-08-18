#!/usr/bin/env bash
# testnet-preflight.sh — check a live-testnet config is ready to deploy, and top
# up the keeper from the deployer so the faucet only has to fund ONE address per
# chain.
#
#   bash scripts/testing/testnet-preflight.sh [config] [--fund]
#
# Without --fund it only reports (safe to run any time). With --fund it sends
# real transactions on the configured chains: deployer -> keeper, KEEPER_TOPUP
# wei per chain, and only when the keeper is below that.
#
# Checks, in order:
#   1. every RPC answers and reports the chain id the config claims
#   2. the deployer holds enough gas for a deploy on each chain
#   3. the keeper holds enough gas to submit claims (topped up with --fund)
#   4. the finality buffer is not the local-dev opt-out (finding H1)
set -euo pipefail

CFG="${1:-scripts/testnet.config.local}"
[[ "${1:-}" == "--fund" ]] && { CFG="scripts/testnet.config.local"; set -- "$CFG" "--fund"; }
FUND=false
for a in "$@"; do [[ "$a" == "--fund" ]] && FUND=true; done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
[[ -f "$CFG" ]] || { echo "no such config: $CFG" >&2; exit 1; }
# shellcheck disable=SC1090
source "$CFG"
export PATH="${FOUNDRY_BIN:-$HOME/.foundry/bin}:$PATH"

# A full deploy is Gate + TestToken + SwapPool + SwapRouter plus the wiring
# transactions. Measured well under 0.001 ETH on both L2s; ask for 10x that so a
# gas spike doesn't strand a half-deployed mesh.
DEPLOY_MIN=${DEPLOY_MIN:-10000000000000000}   # 0.01 ETH
KEEPER_MIN=${KEEPER_MIN:-2000000000000000}    # 0.002 ETH
KEEPER_TOPUP=${KEEPER_TOPUP:-3000000000000000} # 0.003 ETH

# What the DEPLOYER actually needs depends on what this run will do, and those
# are very different numbers. Once a mesh is deployed and the config pins its
# addresses (DEPLOY_* all false), the deployer broadcasts nothing at startup —
# the keeper is the only account that spends, and it has its own check below.
# Demanding deploy money anyway reports a perfectly runnable stack as NOT READY
# and sends the operator to a faucet for gas nothing is going to spend.
#
# Defaults match run.sh: unset means true, so an incomplete config is held to the
# HIGHER bar rather than waved through.
if [[ -n "${DEPLOY:-}" ]]; then
  DEPLOY_TOKENS="${DEPLOY_TOKENS:-$DEPLOY}"; DEPLOY_BRIDGE="${DEPLOY_BRIDGE:-$DEPLOY}"; DEPLOY_SWAP="${DEPLOY_SWAP:-$DEPLOY}"
fi
DEPLOY_TOKENS="${DEPLOY_TOKENS:-true}"; DEPLOY_BRIDGE="${DEPLOY_BRIDGE:-true}"; DEPLOY_SWAP="${DEPLOY_SWAP:-true}"
if [[ "$DEPLOY_TOKENS" == "true" || "$DEPLOY_BRIDGE" == "true" || "$DEPLOY_SWAP" == "true" ]]; then
  DEPLOYER_MIN="$DEPLOY_MIN"
  DEPLOY_MODE="deploying (tokens=$DEPLOY_TOKENS bridge=$DEPLOY_BRIDGE swap=$DEPLOY_SWAP)"
else
  # Run-only: the deployer's remaining job is to top the keeper up, so it needs
  # to be able to afford one transfer.
  DEPLOYER_MIN="$KEEPER_TOPUP"
  DEPLOY_MODE="run-only (contracts already deployed; deployer broadcasts nothing)"
fi

DEPLOYER=$(cast wallet address "$DEPLOYER_KEY")
KEEPER=$(cast wallet address "$KEEPER_KEY")

# Render wei as ETH without bc (not installed everywhere).
eth() { printf '%s' "$(cast from-wei "$1" 2>/dev/null || echo '?')"; }

fail=0
note() { printf '  %s\n' "$*"; }

echo "config    : $CFG"
echo "deployer  : $DEPLOYER   <- fund this one"
echo "keeper    : $KEEPER   <- topped up from the deployer with --fund"
for i in "${!VALIDATOR_KEYS[@]}"; do
  echo "validator$((i+1)): $(cast wallet address "${VALIDATOR_KEYS[$i]}")   (signs off-chain, needs no gas)"
done
echo

# --- 4. finality posture, before anything else: it is the one setting whose
#        wrong value is silently unsafe rather than loudly broken.
if [[ "${SOURCE_ALLOW_ZERO_CONFIRMATION:-false}" == "true" || "${SOURCE_BLOCK_CONFIRMATION:-0}" -eq 0 ]]; then
  echo "REFUSING: SOURCE_BLOCK_CONFIRMATION=${SOURCE_BLOCK_CONFIRMATION:-0}" \
       "allow_zero=${SOURCE_ALLOW_ZERO_CONFIRMATION:-false}"
  note "Signing a Sent event at the tip of a reorg-capable chain lets the"
  note "destination pay out against a deposit that can still vanish. The"
  note "zero-confirmation opt-out is for instant-final local chains only."
  exit 1
fi
echo "finality  : source=${SOURCE_BLOCK_CONFIRMATION} refund=${REFUND_BLOCK_CONFIRMATION} confirmations (zero opt-out off) OK"
echo "mode      : $DEPLOY_MODE"
echo "deployer needs $(eth "$DEPLOYER_MIN") ETH per chain; keeper needs $(eth "$KEEPER_MIN") ETH"
echo

for entry in "${CHAINS[@]}"; do
  IFS='|' read -r cid name rpc _ <<<"$entry"
  echo "--- $name ($cid)"

  # --- 1. the RPC is up AND is the chain we think it is. A config pointed at
  #        the wrong network deploys a perfectly good bridge nobody can use.
  live=$(timeout 20 cast chain-id --rpc-url "$rpc" 2>/dev/null || echo "")
  if [[ -z "$live" ]]; then
    note "UNREACHABLE: $rpc"; fail=1; continue
  fi
  if [[ "$live" != "$cid" ]]; then
    note "CHAIN ID MISMATCH: config says $cid, $rpc reports $live"; fail=1; continue
  fi
  gp=$(timeout 20 cast gas-price --rpc-url "$rpc" 2>/dev/null || echo 0)
  note "rpc ok, block $(cast block-number --rpc-url "$rpc"), gas price ${gp} wei"

  # --- 2. deployer balance
  dbal=$(timeout 20 cast balance "$DEPLOYER" --rpc-url "$rpc" 2>/dev/null || echo 0)
  if (( $(printf '%s' "$dbal") < DEPLOYER_MIN )); then
    note "DEPLOYER UNDERFUNDED: $(eth "$dbal") ETH, need $(eth "$DEPLOYER_MIN") — use a faucet"
    fail=1
  else
    note "deployer $(eth "$dbal") ETH OK"
  fi

  # --- 3. keeper balance, topped up from the deployer on request
  kbal=$(timeout 20 cast balance "$KEEPER" --rpc-url "$rpc" 2>/dev/null || echo 0)
  if (( $(printf '%s' "$kbal") < KEEPER_MIN )); then
    if [[ "$FUND" == "true" ]] && (( $(printf '%s' "$dbal") >= KEEPER_TOPUP )); then
      note "keeper $(eth "$kbal") ETH — sending $(eth "$KEEPER_TOPUP") ETH from the deployer"
      cast send "$KEEPER" --value "$KEEPER_TOPUP" \
        --private-key "$DEPLOYER_KEY" --rpc-url "$rpc" >/dev/null
      kbal=$(cast balance "$KEEPER" --rpc-url "$rpc")
      note "keeper now $(eth "$kbal") ETH"
    else
      note "KEEPER UNDERFUNDED: $(eth "$kbal") ETH, need $(eth "$KEEPER_MIN") — re-run with --fund"
      fail=1
    fi
  else
    note "keeper $(eth "$kbal") ETH OK"
  fi
done

# --- Solana leg, when a devnet config is present. Checked here rather than in a
#     script of its own so one command answers "can I start?" for every chain.
SOL_CFG="${SOL_CFG:-scripts/solana-devnet.config.local}"
if [[ -f "$SOL_CFG" ]]; then
  # shellcheck disable=SC1090
  source "$SOL_CFG"
  echo "--- Solana $SOLANA_CLUSTER (bridge chain id $SOLANA_CHAIN_ID)"

  rpc_post() {
    curl -s -m 20 -X POST "$SOLANA_RPC" -H 'content-type: application/json' -d "$1" 2>/dev/null
  }
  slot=$(rpc_post '{"jsonrpc":"2.0","id":1,"method":"getSlot"}' | grep -oE '"result":[0-9]+' | cut -d: -f2)
  if [[ -z "$slot" ]]; then
    note "UNREACHABLE: $SOLANA_RPC"; fail=1
  else
    note "rpc ok, slot $slot"

    [[ -f "$SOLANA_PAYER_KEYPAIR" ]] \
      || { note "MISSING KEYPAIR: $SOLANA_PAYER_KEYPAIR"; fail=1; }

    lam=$(rpc_post "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getBalance\",\"params\":[\"$SOLANA_PAYER_PUBKEY\"]}" \
          | grep -oE '"value":[0-9]+' | cut -d: -f2)
    lam=${lam:-0}
    # 1 SOL = 1e9 lamports; print with 9 decimals without needing bc.
    printf -v sol '%d.%09d' $((lam/1000000000)) $((lam%1000000000))
    if (( lam < SOLANA_MIN_LAMPORTS )); then
      printf -v need '%d.%09d' $((SOLANA_MIN_LAMPORTS/1000000000)) $((SOLANA_MIN_LAMPORTS%1000000000))
      note "PAYER UNDERFUNDED: $sol SOL, need $need — airdrop to $SOLANA_PAYER_PUBKEY"
      fail=1
    else
      note "payer $sol SOL OK"
    fi

    # `finalized` is the only commitment a fork cannot take back. The relayer
    # enforces this too; catching it here saves a confusing startup failure.
    if [[ "${SOLANA_COMMITMENT:-finalized}" != "finalized" ]]; then
      note "COMMITMENT ${SOLANA_COMMITMENT}: a fork can still discard what we sign"
      fail=1
    else
      note "commitment finalized OK"
    fi

    [[ -n "${SOLANA_PROGRAM_ID:-}" ]] \
      && note "program $SOLANA_PROGRAM_ID" \
      || note "program not deployed yet (cargo build-sbf + solana program deploy)"
  fi
  echo
fi

if [[ "$fail" -ne 0 ]]; then
  echo "NOT READY — fund the addresses above, then re-run with --fund."
  exit 1
fi
echo "READY. Deploy and run the EVM mesh with:"
echo "  bash scripts/run.sh $CFG"
