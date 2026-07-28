#!/usr/bin/env bash
# Phase C — same-chain SwapPool end-to-end on a local anvil.
#
# Boots one anvil, deploys a 6-dec stablecoin hub + two 18-dec tokens + the
# SwapPool (via script/DeploySwap.s.sol), seeds reserves, then exercises real
# swaps with `cast` and asserts:
#   1. a decimals-crossing swap (WETH -> mUSD) pays the pegged rate,
#   2. stable -> token round-trips the rate,
#   3. a swap that would exceed the output token's locked reserve REVERTS
#      (the "max swap up to token lock" rule).
#
# Run from anywhere:  bash scripts/testing/swap.sh
set -euo pipefail

export PATH="$HOME/.foundry/bin:$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CONTRACTS="$ROOT/contracts"
LOGS="$ROOT/.swap-logs"
mkdir -p "$LOGS"

# --- anvil default account 0 (deployer / owner / liquidity provider) ---
ACC0=0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
KEY0=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
# a distinct swapper (anvil account 1)
USER=0x70997970C51812dc3A010C7d01b50e0d17dc79C8
USER_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d

RPC=http://127.0.0.1:8545
CHAIN=1337

MAX_UINT=0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff

cleanup() {
  echo "--- cleaning up ---"
  [[ -n "${ANVIL_PID:-}" ]] && kill "$ANVIL_PID" 2>/dev/null || true
}
trap cleanup EXIT

# read a decimal uint from a `cast call` (strip any scientific/[..] suffix)
bal() { cast call "$1" "balanceOf(address)(uint256)" "$2" --rpc-url $RPC | awk '{print $1}'; }

echo "=== killing stale anvil ==="
pkill -f "anvil --chain-id" 2>/dev/null || true
sleep 1

echo "=== starting anvil ($CHAIN) ==="
anvil --chain-id $CHAIN --port 8545 >"$LOGS/anvil.log" 2>&1 &
ANVIL_PID=$!
for i in $(seq 1 50); do
  cast chain-id --rpc-url "$RPC" >/dev/null 2>&1 && break
  sleep 0.2
done
echo "  chainId: $(cast chain-id --rpc-url $RPC)"

echo "=== deploying + seeding via forge script ==="
cd "$CONTRACTS"
forge script script/DeploySwap.s.sol:DeploySwap \
  --rpc-url "$RPC" --private-key $KEY0 --broadcast \
  >"$LOGS/deploy.log" 2>&1 || { echo "!! deploy failed"; tail -30 "$LOGS/deploy.log"; exit 1; }

source "$CONTRACTS/fixtures/swap-deploy.env"
echo "  SwapPool=$SWAP_POOL"
echo "  stable=$STABLE  WETH=$WETH  TT=$TT"

FAIL=0
check() { # $1=label $2=got $3=want
  if [[ "$2" == "$3" ]]; then echo "  ✅ $1: $2"; else echo "  ❌ $1: got $2 want $3"; FAIL=1; fi
}

# ---------------------------------------------------------------------------
echo "=== test 1: WETH -> mUSD (decimals-crossing, pegged rate) ==="
# fund USER with 1 WETH and approve the pool
cast send "$WETH" "mint(address,uint256)" "$USER" 1000000000000000000 --rpc-url $RPC --private-key $KEY0 >/dev/null
cast send "$WETH" "approve(address,uint256)" "$SWAP_POOL" $MAX_UINT --rpc-url $RPC --private-key $USER_KEY >/dev/null
# swap(tokenIn, tokenOut, amountIn, minOut, to)
cast send "$SWAP_POOL" "swap(address,address,uint256,uint256,address)" \
  "$WETH" "$STABLE" 1000000000000000000 0 "$USER" \
  --rpc-url $RPC --private-key $USER_KEY >/dev/null
check "USER mUSD balance" "$(bal $STABLE $USER)" "3180000000"   # 3180.000000 (6 dec)

# ---------------------------------------------------------------------------
echo "=== test 2: mUSD -> TT (stable -> token) ==="
# swap the 3180 mUSD USER just got into TT (price 1.0) -> expect 3180 TT (18 dec)
cast send "$STABLE" "approve(address,uint256)" "$SWAP_POOL" $MAX_UINT --rpc-url $RPC --private-key $USER_KEY >/dev/null
cast send "$SWAP_POOL" "swap(address,address,uint256,uint256,address)" \
  "$STABLE" "$TT" 3180000000 0 "$USER" \
  --rpc-url $RPC --private-key $USER_KEY >/dev/null
check "USER TT balance" "$(bal $TT $USER)" "3180000000000000000000"  # 3180e18

# ---------------------------------------------------------------------------
echo "=== test 3: over-the-lock swap must REVERT ==="
# After test 1, the WETH reserve is 101e18 (100 seeded + 1 swapped in). Buying
# ~125.8 WETH needs 400,000 mUSD -> exceeds the lock -> ExceedsLock.
cast send "$STABLE" "mint(address,uint256)" "$USER" 400000000000 --rpc-url $RPC --private-key $KEY0 >/dev/null
if cast send "$SWAP_POOL" "swap(address,address,uint256,uint256,address)" \
     "$STABLE" "$WETH" 400000000000 0 "$USER" \
     --rpc-url $RPC --private-key $USER_KEY >/dev/null 2>&1; then
  echo "  ❌ over-lock swap SUCCEEDED (should have reverted)"; FAIL=1
else
  echo "  ✅ over-lock swap reverted (ExceedsLock)"
fi
# the revert left the reserve untouched (101e18: 100 seeded + 1 from test 1)
MAXOUT=$(cast call "$SWAP_POOL" "maxSwapOut(address)(uint256,uint256)" "$WETH" --rpc-url $RPC | head -1 | awk '{print $1}')
check "WETH reserve (lock) intact" "$MAXOUT" "101000000000000000000"  # 101e18

echo
echo "================= RESULT ================="
if [[ "$FAIL" == "0" ]]; then
  echo "✅ Phase C PASS: pegged swaps priced correctly + reserve lock enforced on-chain"
else
  echo "❌ FAIL — see $LOGS/"
  exit 1
fi
echo "=========================================="
