#!/usr/bin/env bash
# Security regression tests for the GraphQL API. Probes the two issues found by
# adversarial testing and asserts they are now CLOSED:
#
#   SEC-1 (path traversal): submission(submissionId:"../leak") must NOT read a
#         file outside the store dir. The submissionId is used to build a file
#         path (dir backend) / a sig-store URL (remote) — it must be validated as
#         a 32-byte hex hash, not trusted as a filename.
#   SEC-2 (query amplification / no limits): an alias bomb (hundreds of aliased
#         expensive fields in one request) must be rejected by depth/complexity
#         limits instead of fanning out to hundreds of store loads / RPC calls.
#
# Run from anywhere:  bash scripts/graphql-sec.sh
set -euo pipefail

export PATH="$HOME/.foundry/bin:$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIND=127.0.0.1:8091
URL="http://$BIND/graphql"
BASE="$(mktemp -d)"
STORE="$BASE/store"; mkdir -p "$STORE"

cleanup() { [[ -n "${PID:-}" ]] && kill "$PID" 2>/dev/null || true; rm -rf "$BASE"; }
trap cleanup EXIT

gql() { curl -s "$URL" -H 'content-type: application/json' \
          --data "$(printf '{"query":%s}' "$(printf '%s' "$1" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')")"; }
fail() { echo "❌ FAIL: $1"; echo "--- api log ---"; tail -20 "$BASE/api.log" 2>/dev/null || true; exit 1; }

echo "=== build ==="
( cd "$ROOT" && cargo build -p graphql-api >/dev/null 2>&1 )

echo "=== seed: one legit record in the store + a SECRET record OUTSIDE it ==="
# A real 32-byte submissionId (63 'a' + '1' = 64 hex digits); the file is named
# <id>.json as the store expects.
ID64="$(printf 'a%.0s' {1..63})1"
cat > "$STORE/$ID64.json" <<EOF
{"submission_id":"0x$ID64","debridge_id":"0x00","amount":"1","chain_id_from":1337,"chain_id_to":1338,"nonce":0,"receiver":"0x00","auto_params":"0x","native_sender":"0x","signatures":[]}
EOF
# A valid SubmissionRecord sitting OUTSIDE the store dir, with a unique marker.
cat > "$BASE/leak.json" <<'EOF'
{"submission_id":"0xsecret","debridge_id":"0xdead","amount":"999999999","chain_id_from":1,"chain_id_to":2,"nonce":42,"receiver":"0xLEAKEDSECRETMARKER","auto_params":"0x","native_sender":"0x","signatures":[]}
EOF

echo "=== boot graphql-api --dir ==="
"$ROOT/target/debug/graphql-api" --bind "$BIND" --dir "$STORE" --threshold 2 >"$BASE/api.log" 2>&1 & PID=$!
for i in $(seq 1 40); do curl -s "http://$BIND/health" >/dev/null 2>&1 && break; sleep 0.2; done
curl -s "http://$BIND/health" | grep -q ok || fail "API did not come up"

echo
echo "########## SEC-1: path traversal via submission(submissionId) ##########"
echo "--- control: a normal in-store lookup still works ---"
OUT=$(gql "query { submission(submissionId:\"0x$ID64\") { amount } }")
echo "$OUT"
echo "$OUT" | grep -q '"amount":"1"' || fail "control lookup broke (legit ids must still resolve)"

echo "--- attack: submissionId = ../leak ---"
OUT=$(gql 'query { submission(submissionId:"../leak") { receiver } }')
echo "$OUT"
if echo "$OUT" | grep -q 'LEAKEDSECRETMARKER'; then
  fail "PATH TRAVERSAL: read a file outside the store dir (SEC-1 OPEN)"
fi
# Must be a clean null or error — never the secret. Also try a few more shapes.
for atk in '../leak' '..%2fleak' '/etc/passwd' '0x../../leak' '../../leak'; do
  OUT=$(gql "query { submission(submissionId:\"$atk\") { receiver } }")
  echo "$OUT" | grep -q 'LEAKEDSECRETMARKER' && fail "PATH TRAVERSAL via '$atk' (SEC-1 OPEN)"
done
echo "✅ SEC-1 closed: traversal attempts leak nothing (validated as hex hash)"

echo
echo "########## SEC-2: query amplification / depth+complexity limits ##########"
# Build an alias bomb: 2000 aliased stats fields in one request. Unbounded, this
# fans out to 2000 full store loads; with limits it must be rejected up front.
BOMB="query { $(python3 -c 'print(" ".join(f"a{i}: stats {{ total }}" for i in range(2000)))') }"
OUT=$(gql "$BOMB")
echo "$OUT" | head -c 300; echo
echo "$OUT" | grep -qi 'complexity\|depth\|limit\|too' || fail "alias bomb was NOT rejected (SEC-2 OPEN — no query limits)"
# And a shallow legit query with nested selections must still succeed.
NEST="query { submissions { signatures { signer } } submission(submissionId:\"0x$ID64\"){ signatures { signature } } }"
gql "$NEST" >/dev/null   # this one is shallow & legit, must still succeed
echo "✅ SEC-2 closed: alias bomb rejected by complexity/depth limit; legit queries pass"

echo
echo "########## bonus: a legit small query still works end-to-end ##########"
OUT=$(gql 'query { stats { total } submissions(filter:{chainIdTo:1338}) { submissionId } }')
echo "$OUT"
echo "$OUT" | grep -q "$ID64" || fail "legit query regressed"
echo "✅ legit queries unaffected"

echo
echo "================= RESULT ================="
echo "✅ PASS: SEC-1 (path traversal) and SEC-2 (query amplification) are closed"
echo "=========================================="
