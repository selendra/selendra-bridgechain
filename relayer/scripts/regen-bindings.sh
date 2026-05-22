#!/usr/bin/env bash
# Regenerate Go bindings for BeefyClient.sol + Gateway.sol.
#
# Run from the relayer/ directory. Requires:
#   - forge (Foundry) on PATH
#   - abigen on PATH (`go install github.com/ethereum/go-ethereum/cmd/abigen@v1.17.3`)
#   - python3 (for stripping the abi field out of forge's JSON output)
set -euo pipefail

RELAYER_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CONTRACTS_DIR="$(cd "$RELAYER_DIR/../contracts" && pwd)"
OUT_DIR="$RELAYER_DIR/internal/bindings"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$OUT_DIR"

(cd "$CONTRACTS_DIR" && forge build)

# Forge emits a combined JSON with `abi`, `bytecode`, `metadata`, …
# abigen wants the ABI as a JSON array and bytecode as a plain hex string.
# The bytecode lets abigen generate a `Deploy<Name>` function so the e2e
# tests can spin up fresh contracts against Anvil from pure Go.
for contract in BeefyClient Gateway; do
    python3 -c "
import json
data = json.load(open('$CONTRACTS_DIR/out/${contract}.sol/${contract}.json'))
open('$TMP/${contract}.abi.json', 'w').write(json.dumps(data['abi']))
bin = data['bytecode']['object']
if bin.startswith('0x'): bin = bin[2:]
open('$TMP/${contract}.bin', 'w').write(bin)
"
done

abigen --abi "$TMP/BeefyClient.abi.json" --bin "$TMP/BeefyClient.bin" \
    --pkg bindings --type BeefyClient --out "$OUT_DIR/beefy_client.go"
abigen --abi "$TMP/Gateway.abi.json" --bin "$TMP/Gateway.bin" \
    --pkg bindings --type Gateway --out "$OUT_DIR/gateway.go"

echo "regen-bindings: wrote $OUT_DIR/{beefy_client.go,gateway.go}"
