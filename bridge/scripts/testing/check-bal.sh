#!/usr/bin/env bash
set -euo pipefail
export PATH="$HOME/.foundry/bin:$PATH"
TOKEN=0x5FbDB2315678afecb367f032d93F642f64180aa3
ACC0=0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
GATE=0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512
for pair in "A http://127.0.0.1:8545" "B http://127.0.0.1:8546"; do
  set -- $pair; LABEL=$1; RPC=$2
  echo "--- chain $LABEL ($RPC) ---"
  echo "  chain-id     : $(cast chain-id --rpc-url "$RPC" 2>/dev/null || echo DOWN)"
  echo "  token symbol : $(cast call "$TOKEN" 'symbol()(string)' --rpc-url "$RPC" 2>/dev/null || echo none)"
  echo "  acc0 TST     : $(cast call "$TOKEN" 'balanceOf(address)(uint256)' "$ACC0" --rpc-url "$RPC" 2>/dev/null || echo err)"
  echo "  gate TST     : $(cast call "$TOKEN" 'balanceOf(address)(uint256)' "$GATE" --rpc-url "$RPC" 2>/dev/null || echo err)"
  echo "  acc0 ETH     : $(cast balance "$ACC0" --rpc-url "$RPC" 2>/dev/null || echo err)"
done
