#!/usr/bin/env bash
# Full end-to-end test: bridgechain + Anvil + relayer driver.
#
# 1. Build the inject-msg helper (standalone Rust crate).
# 2. Start a dev bridgechain node with offchain indexing.
# 3. Start anvil with a 1s block time so randao delay elapses quickly.
# 4. Run `go test -tags e2e` against the driver package.
# 5. Tear everything down.
#
# Run from anywhere — paths are absolute.

set -euo pipefail

RELAYER_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REPO_DIR="$(cd "$RELAYER_DIR/.." && pwd)"
NODE_BIN="$REPO_DIR/target/release/solochain-template-node"
INJECT_MSG_DIR="$REPO_DIR/tools/inject-msg"
INJECT_MSG_BIN="$INJECT_MSG_DIR/target/release/inject-msg"

export PATH="$HOME/.cargo/bin:$HOME/.local/go/bin:$HOME/.foundry/bin:$PATH"

if [[ ! -x "$NODE_BIN" ]]; then
    echo "e2e: node binary missing at $NODE_BIN" >&2
    echo "    build with: cargo build --release -p solochain-template-node" >&2
    exit 1
fi
if ! command -v anvil >/dev/null; then
    echo "e2e: anvil not on PATH (foundryup)" >&2
    exit 1
fi
if ! command -v cargo >/dev/null; then
    echo "e2e: cargo not on PATH — needed to build inject-msg" >&2
    exit 1
fi

echo "e2e: building inject-msg helper..."
(cd "$INJECT_MSG_DIR" && cargo build --release --quiet)
if [[ ! -x "$INJECT_MSG_BIN" ]]; then
    echo "e2e: inject-msg build did not produce $INJECT_MSG_BIN" >&2
    exit 1
fi
export INJECT_MSG_BIN

TMPDIR="$(mktemp -d -t bridgechain-e2e.XXXXXX)"
NODE_LOG="$TMPDIR/node.log"
ANVIL_LOG="$TMPDIR/anvil.log"
echo "e2e: tmp $TMPDIR"

NODE_PID=""
ANVIL_PID=""
cleanup() {
    for pid in $NODE_PID $ANVIL_PID; do
        if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done
    rm -rf "$TMPDIR"
}
trap cleanup EXIT INT TERM

# ─── bridgechain ────────────────────────────────────────────────────────
"$NODE_BIN" \
    --dev \
    --base-path "$TMPDIR/node" \
    --rpc-port 9944 \
    --port 30333 \
    --rpc-cors all \
    --no-prometheus \
    --no-telemetry \
    --enable-offchain-indexing=true \
    > "$NODE_LOG" 2>&1 &
NODE_PID=$!
echo "e2e: bridgechain pid $NODE_PID"

# ─── anvil ──────────────────────────────────────────────────────────────
# 1s block time keeps the randao delay short. Default chainID 31337.
anvil \
    --port 8545 \
    --block-time 1 \
    --silent \
    > "$ANVIL_LOG" 2>&1 &
ANVIL_PID=$!
echo "e2e: anvil pid $ANVIL_PID"

# ─── wait for both to be ready ──────────────────────────────────────────
for i in $(seq 1 30); do
    if grep -q "BEEFY pallet available\|run BEEFY worker" "$NODE_LOG" 2>/dev/null; then
        break
    fi
    sleep 1
done
if ! grep -q "BEEFY" "$NODE_LOG"; then
    echo "e2e: bridgechain didn't reach BEEFY readiness — last 40 lines:" >&2
    tail -40 "$NODE_LOG" >&2
    exit 1
fi

for i in $(seq 1 15); do
    if curl -s -X POST -H "Content-Type: application/json" \
        --data '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}' \
        http://127.0.0.1:8545 2>/dev/null | grep -q '"result"'; then
        break
    fi
    sleep 1
done

echo "e2e: running go test -tags e2e..."
export E2E_ETH_RPC="ws://127.0.0.1:8545"
export E2E_SUB_RPC="ws://127.0.0.1:9944"

cd "$RELAYER_DIR"
# Match both TestE2E_BeefyRelay* and TestE2E_FullMessagePassing.
if go test -tags e2e -v -timeout 480s -run "TestE2E_" ./internal/driver/; then
    echo
    echo "e2e: PASS"
else
    echo
    echo "e2e: FAIL — last 60 lines of each log:" >&2
    echo "── bridgechain ──" >&2
    tail -60 "$NODE_LOG" >&2
    echo "── anvil ──" >&2
    tail -60 "$ANVIL_LOG" >&2
    exit 1
fi
