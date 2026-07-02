#!/usr/bin/env bash
# Launch the bridge backend (graphql-api) + frontend (vite dev) as detached
# services that survive past this shell, then verify both are up.
set -euo pipefail

export PATH="$HOME/.nvm/versions/node/v25.9.0/bin:$HOME/.cargo/bin:$PATH"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG=/tmp/bridge-run
mkdir -p "$LOG"

# Free the ports if a previous run is still holding them.
fuser -k 8088/tcp 5173/tcp 2>/dev/null || true
sleep 0.5

# --- backend: GraphQL API over the existing signature store ---
cd "$ROOT"
setsid bash -c 'exec ./target/debug/graphql-api --bind 127.0.0.1:8088 --dir sig-store-data --threshold 2 --chains-file chains.json' \
  >"$LOG/api.log" 2>&1 < /dev/null &
disown || true

# --- frontend: Vite dev server (proxies /graphql -> 127.0.0.1:8088) ---
cd "$ROOT/web"
setsid bash -c 'exec npx vite --host 127.0.0.1 --port 5173 --strictPort' \
  >"$LOG/web.log" 2>&1 < /dev/null &
disown || true

# --- wait for health ---
for _ in $(seq 1 40); do curl -s http://127.0.0.1:8088/health >/dev/null 2>&1 && break; sleep 0.25; done
for _ in $(seq 1 80); do curl -s http://127.0.0.1:5173/         >/dev/null 2>&1 && break; sleep 0.25; done

echo "=== backend /health ==="
curl -s http://127.0.0.1:8088/health; echo
echo "=== frontend index.html ==="
curl -s http://127.0.0.1:5173/ | grep -oE 'id="root"|/src/main.tsx' | sort -u
echo "=== stats via dev proxy ==="
curl -s http://127.0.0.1:5173/graphql -H 'content-type: application/json' \
  --data '{"query":"{ stats { total signed ready threshold } }"}'; echo
echo "=== running pids ==="
pgrep -af 'graphql-api|vite' | grep -v pgrep || true
