#!/usr/bin/env bash
# stop.sh — tear down everything scripts/run.sh started, using the same config
# (for the ports and the Postgres container name).
#
#   bash scripts/stop.sh [config-file]     # default: scripts/run.config
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="${1:-$ROOT/scripts/run.config}"
[[ -f "$CONFIG" ]] && source "$CONFIG"

# defaults in case the config is missing a field
STORE_PORT="${STORE_PORT:-8080}"
GQL_PORT="${GQL_PORT:-8088}"
WEB_PORT="${WEB_PORT:-5173}"
PG_NAME="${PG_NAME:-bridge-run-pg}"
PG_DOCKER="${PG_DOCKER:-true}"

echo "=== stopping bridge stack ==="

# Free the service ports: every chain's anvil RPC port + store/gql/web.
ports=( "$STORE_PORT" "$GQL_PORT" "$WEB_PORT" )
for entry in "${CHAINS[@]:-}"; do
  [[ -z "$entry" ]] && continue
  IFS='|' read -r _ _ rpc _ <<<"$entry"
  rpc="${rpc// /}"
  [[ -n "$rpc" ]] && ports+=( "${rpc##*:}" )
done
fuser -k "${ports[@]/%//tcp}" 2>/dev/null || true

# Kill the long-lived services by name (some don't hold a port).
for p in 'anvil --chain-id' 'target/debug/sig-store' 'target/debug/validator' \
         'target/debug/keeper' 'target/debug/indexer' 'target/debug/graphql-api' \
         'vite'; do
  pkill -f "$p" 2>/dev/null || true
done

# Remove the Postgres container.
if [[ "$PG_DOCKER" == "true" ]] && command -v docker >/dev/null 2>&1; then
  docker rm -f "$PG_NAME" >/dev/null 2>&1 || true
fi

echo "  stopped (ports ${ports[*]} freed; container $PG_NAME removed if present)"
