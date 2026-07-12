#!/usr/bin/env bash
# Phase D — the GraphQL read view over a same-chain SwapPool.
#
# Boots one anvil, deploys + seeds the pool (script/DeploySwap.s.sol), then starts
# graphql-api with `--swap CHAINID=RPC,POOL` and asserts:
#   1. `pools(chainId)` lists the stable + both tokens with the seeded reserves,
#      prices, decimals, isStable flag, and the derived maxSwapUsd,
#   2. `swapQuote(...)` matches the on-chain `SwapPool.quote` (the pegged rate),
#   3. an unconfigured chain / unlisted token degrades to null (never an error).
#
# Read-only: the API never executes a swap. Run from anywhere:  bash scripts/swap-gql.sh
set -euo pipefail

export PATH="$HOME/.foundry/bin:$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS="$ROOT/contracts"
LOGS="$ROOT/.swap-logs"; mkdir -p "$LOGS"
STORE="$(mktemp -d)"                 # graphql-api needs a store even for pool reads
BIND=127.0.0.1:8089
URL="http://$BIND/graphql"

KEY0=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
RPC=http://127.0.0.1:8545
CHAIN=1337

cleanup() {
  [[ -n "${API_PID:-}" ]] && kill "$API_PID" 2>/dev/null || true
  [[ -n "${ANVIL_PID:-}" ]] && kill "$ANVIL_PID" 2>/dev/null || true
  rm -rf "$STORE"
}
trap cleanup EXIT

gql() { curl -s "$URL" -H 'content-type: application/json' \
          --data "$(printf '{"query":%s}' "$(printf '%s' "$1" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')")"; }
field() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
FAIL=0
check() { if [[ "$2" == "$3" ]]; then echo "  ✅ $1: $2"; else echo "  ❌ $1: got $2 want $3"; FAIL=1; fi; }
fail() { echo "❌ FAIL: $1"; echo "--- api log ---"; tail -20 "$LOGS/graphql-swap.log" || true; exit 1; }

echo "=== killing stale anvil ==="
pkill -f "anvil --chain-id" 2>/dev/null || true; sleep 1

echo "=== starting anvil ($CHAIN) ==="
anvil --chain-id $CHAIN --port 8545 >"$LOGS/anvil-gql.log" 2>&1 & ANVIL_PID=$!
for i in $(seq 1 50); do cast chain-id --rpc-url "$RPC" >/dev/null 2>&1 && break; sleep 0.2; done

echo "=== deploying + seeding SwapPool ==="
cd "$CONTRACTS"
forge script script/DeploySwap.s.sol:DeploySwap --rpc-url "$RPC" --private-key $KEY0 --broadcast \
  >"$LOGS/deploy-gql.log" 2>&1 || { echo "!! deploy failed"; tail -30 "$LOGS/deploy-gql.log"; exit 1; }
source "$CONTRACTS/fixtures/swap-deploy.env"
echo "  SwapPool=$SWAP_POOL  stable=$STABLE  WETH=$WETH  TT=$TT"

echo "=== build + boot graphql-api (--swap $CHAIN=$RPC,$SWAP_POOL) ==="
( cd "$ROOT" && cargo build -p graphql-api >/dev/null 2>&1 )
"$ROOT/target/debug/graphql-api" --bind "$BIND" --dir "$STORE" \
  --swap "$CHAIN=$RPC,$SWAP_POOL" >"$LOGS/graphql-swap.log" 2>&1 & API_PID=$!
for i in $(seq 1 40); do curl -s "http://$BIND/health" >/dev/null 2>&1 && break; sleep 0.25; done
curl -s "http://$BIND/health" | grep -q ok || fail "API never came up"
echo "✅ API up"

# ---------------------------------------------------------------------------
echo
echo "=== Q1: pools(chainId:$CHAIN) — listed tokens, reserves, prices ==="
OUT=$(gql "query { pools(chainId:$CHAIN) { token symbol decimals price reserve maxSwapUsd isStable } }")
echo "$OUT"
# helper: pull a field for the token whose symbol == $1
pick() { echo "$OUT" | field "next(p['$2'] for p in d['data']['pools'] if p['symbol']=='$1')"; }
check "pool count"        "$(echo "$OUT" | field 'len(d["data"]["pools"])')" "3"
check "mUSD isStable"     "$(pick mUSD isStable)"   "True"
check "mUSD decimals"     "$(pick mUSD decimals)"   "6"
check "mUSD price"        "$(pick mUSD price)"      "1000000000000000000"          # PRICE_ONE
check "mUSD reserve"      "$(pick mUSD reserve)"    "1000000000000"                # 1_000_000e6
check "WETH isStable"     "$(pick WETH isStable)"   "False"
check "WETH decimals"     "$(pick WETH decimals)"   "18"
check "WETH price"        "$(pick WETH price)"      "3180000000000000000000"       # 3180e18
check "WETH reserve"      "$(pick WETH reserve)"    "100000000000000000000"        # 100e18
# maxSwapUsd(WETH) = reserve*price/1e18 = 100e18 * 3180 = 318000e18
check "WETH maxSwapUsd"   "$(pick WETH maxSwapUsd)" "318000000000000000000000"
check "TT reserve"        "$(pick TT reserve)"      "500000000000000000000000"     # 500_000e18

# ---------------------------------------------------------------------------
echo
echo "=== Q2: swapQuote matches on-chain quote (WETH -> mUSD, 1 WETH) ==="
GQ=$(gql "query { swapQuote(chainId:$CHAIN, tokenIn:\"$WETH\", tokenOut:\"$STABLE\", amountIn:\"1000000000000000000\") }" | field 'd["data"]["swapQuote"]')
ONCHAIN=$(cast call "$SWAP_POOL" "quote(address,address,uint256)(uint256)" "$WETH" "$STABLE" 1000000000000000000 --rpc-url $RPC | awk '{print $1}')
check "swapQuote == on-chain" "$GQ" "$ONCHAIN"
check "swapQuote value"       "$GQ" "3180000000"     # 3180.000000 mUSD (6 dec)

# ---------------------------------------------------------------------------
echo
echo "=== Q3: read view degrades to null (never errors) ==="
# unconfigured chain -> null
N1=$(gql "query { pools(chainId:9999) { token } }" | field 'd["data"]["pools"]')
check "pools(unknown chain)" "$N1" "None"
# unlisted token -> quote reverts on-chain -> null
BOGUS=0x000000000000000000000000000000000000dEaD
N2=$(gql "query { swapQuote(chainId:$CHAIN, tokenIn:\"$BOGUS\", tokenOut:\"$STABLE\", amountIn:\"1\") }" | field 'd["data"]["swapQuote"]')
check "swapQuote(unlisted token)" "$N2" "None"

echo
echo "================= RESULT ================="
if [[ "$FAIL" == "0" ]]; then
  echo "✅ Phase D PASS: pools + swapQuote read view serves live pool state"
else
  echo "❌ FAIL — see $LOGS/"
  exit 1
fi
echo "=========================================="
