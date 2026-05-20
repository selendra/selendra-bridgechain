//go:build integration

// Integration tests against a live bridgechain dev node.
//
// Run via `go test -tags integration -v ./internal/beefy`. Expects a node
// running on ws://127.0.0.1:9944 (override with BRIDGECHAIN_SUBSTRATE_RPC).
//
// `scripts/integration-smoke.sh` starts a node, waits for BEEFY to come
// online, runs these tests, and tears down.

package beefy

import (
	"context"
	"os"
	"testing"
	"time"

	"github.com/nathselendra/bridgechain/relayer/internal/substrate"
)

const subscribeTimeout = 30 * time.Second

func nodeRPC() string {
	if v := os.Getenv("BRIDGECHAIN_SUBSTRATE_RPC"); v != "" {
		return v
	}
	return "ws://127.0.0.1:9944"
}

func dial(t *testing.T) (*substrate.Client, context.Context, context.CancelFunc) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), subscribeTimeout)
	cli, err := substrate.Dial(ctx, nodeRPC())
	if err != nil {
		cancel()
		t.Fatalf("dial node at %s: %v", nodeRPC(), err)
	}
	t.Cleanup(func() { cli.Close() })
	return cli, ctx, cancel
}

func TestIntegration_BeefyJustifications(t *testing.T) {
	cli, ctx, cancel := dial(t)
	defer cancel()

	_, stream, err := cli.Subscribe(ctx,
		"beefy_subscribeJustifications", "beefy_unsubscribeJustifications")
	if err != nil {
		t.Fatalf("subscribe: %v", err)
	}

	select {
	case <-ctx.Done():
		t.Fatalf("no justification within %s — is BEEFY producing?", subscribeTimeout)
	case raw, ok := <-stream:
		if !ok {
			t.Fatalf("subscription stream closed unexpectedly")
		}
		t.Logf("received raw notification: %d bytes", len(raw))

		var hexStr string
		if err := unmarshalHex(raw, &hexStr); err != nil {
			t.Fatalf("notification body: %v", err)
		}
		encoded := mustHexDecode(t, hexStr)

		commit, err := DecodeSignedCommitment(encoded)
		if err != nil {
			t.Fatalf("decode commitment: %v", err)
		}

		if len(commit.Commitment.Payload) == 0 {
			t.Fatalf("commitment has zero payload items — expected at least 'mh' (MMR root)")
		}
		var sawMmrRoot bool
		for _, item := range commit.Commitment.Payload {
			if item.ID == ([2]byte{'m', 'h'}) {
				if len(item.Data) != 32 {
					t.Errorf("MMR root payload has length %d, want 32", len(item.Data))
				}
				sawMmrRoot = true
			}
		}
		if !sawMmrRoot {
			t.Errorf("no 'mh' (MMR root) payload item in commitment")
		}
		if commit.SignatureCount() == 0 {
			t.Errorf("commitment has no signatures — single validator should still sign its own")
		}
		t.Logf("commitment OK: block=%d set_id=%d signatures=%d/%d",
			commit.Commitment.BlockNumber, commit.Commitment.ValidatorSetID,
			commit.SignatureCount(), len(commit.Signatures))
	}
}
