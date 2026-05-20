#!/usr/bin/env bash
# End-to-end smoke test: start a bridgechain dev node, wait for BEEFY to
# produce its first justification, run `go test -tags integration`, tear
# everything down.
#
# Run from the relayer/ directory or anywhere — paths are absolute.
set -euo pipefail

RELAYER_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REPO_DIR="$(cd "$RELAYER_DIR/.." && pwd)"
NODE_BIN="$REPO_DIR/target/release/solochain-template-node"

if [[ ! -x "$NODE_BIN" ]]; then
    echo "smoke: node binary not found at $NODE_BIN" >&2
    echo "build it with: cargo build --release -p solochain-template-node" >&2
    exit 1
fi

TMPDIR="$(mktemp -d -t bridgechain-smoke.XXXXXX)"
LOG="$TMPDIR/node.log"
echo "smoke: node tmp $TMPDIR"

cleanup() {
    if [[ -n "${NODE_PID-}" ]] && kill -0 "$NODE_PID" 2>/dev/null; then
        kill "$NODE_PID" 2>/dev/null || true
        wait "$NODE_PID" 2>/dev/null || true
    fi
    rm -rf "$TMPDIR"
}
trap cleanup EXIT INT TERM

# --dev gives us a single Alice validator. --tmp would normally clean up
# state itself but we want the log on disk for diagnostics, so we run with
# an explicit base path.
"$NODE_BIN" \
    --dev \
    --base-path "$TMPDIR/node" \
    --rpc-port 9944 \
    --port 30333 \
    --rpc-cors all \
    --no-prometheus \
    --no-telemetry \
    --enable-offchain-indexing=true \
    > "$LOG" 2>&1 &
NODE_PID=$!
echo "smoke: node pid $NODE_PID"

# Wait for RPC to come up. The node's first log line containing "Idle" or
# "Imported" means it's serving.
for i in $(seq 1 30); do
    if grep -q "Idle\|Imported #1" "$LOG" 2>/dev/null; then
        break
    fi
    sleep 1
done
if ! grep -q "Idle\|Imported" "$LOG"; then
    echo "smoke: node didn't start within 30s — log tail:" >&2
    tail -50 "$LOG" >&2
    exit 1
fi

# BEEFY signing takes a few extra blocks before the first justification.
echo "smoke: waiting for first BEEFY justification..."
for i in $(seq 1 60); do
    if grep -q "Imported justification" "$LOG" 2>/dev/null || \
       grep -qi "beefy" "$LOG" 2>/dev/null; then
        break
    fi
    sleep 1
done

echo "smoke: running go test -tags integration ./..."
export PATH="$HOME/.local/go/bin:$PATH"
export BRIDGECHAIN_SUBSTRATE_RPC="ws://127.0.0.1:9944"

cd "$RELAYER_DIR"
if go test -tags integration -v -timeout 90s ./...; then
    echo
    echo "smoke: PASS"
else
    echo "smoke: FAIL — node log tail:" >&2
    tail -100 "$LOG" >&2
    exit 1
fi
