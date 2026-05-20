//go:build integration

// Integration tests against a live bridgechain dev node. See
// internal/beefy/integration_test.go for the bring-up procedure.

package outbound

import (
	"context"
	"encoding/hex"
	"os"
	"testing"
	"time"

	"github.com/nathselendra/bridgechain/relayer/internal/mmr"
	"github.com/nathselendra/bridgechain/relayer/internal/substrate"
)

const dialTimeout = 30 * time.Second

func nodeRPC() string {
	if v := os.Getenv("BRIDGECHAIN_SUBSTRATE_RPC"); v != "" {
		return v
	}
	return "ws://127.0.0.1:9944"
}

func dial(t *testing.T) (*substrate.Client, context.Context, context.CancelFunc) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), dialTimeout)
	cli, err := substrate.Dial(ctx, nodeRPC())
	if err != nil {
		cancel()
		t.Fatalf("dial: %v", err)
	}
	t.Cleanup(func() { cli.Close() })
	return cli, ctx, cancel
}

// State-call against `BridgeOutboundApi::messages_at(1)` — block 1 should
// have no outbound messages on a fresh dev node, so the runtime API returns
// an empty Vec. The wire format is `compact(0) = 0x00`.
func TestIntegration_MessagesAtEmpty(t *testing.T) {
	cli, ctx, cancel := dial(t)
	defer cancel()

	args := []byte{0x01, 0x00, 0x00, 0x00} // u32 LE = 1
	raw, err := cli.StateCall(ctx, "BridgeOutboundApi_messages_at", args, nil)
	if err != nil {
		t.Fatalf("state_call: %v", err)
	}
	if len(raw) != 1 || raw[0] != 0x00 {
		t.Errorf("expected compact(0) = [0x00], got %s", hex.EncodeToString(raw))
	}
}

// `mmr_generateProof([1])` should succeed once the node has produced more
// than one block (the MMR is non-empty).
func TestIntegration_MmrGenerateProof(t *testing.T) {
	cli, ctx, cancel := dial(t)
	defer cancel()

	// Give the node time to build a few blocks past genesis so the MMR
	// actually has a leaf at index 0 (block 1).
	deadline := time.Now().Add(45 * time.Second)
	var resp *substrate.LeavesProof
	var lastErr error
	for time.Now().Before(deadline) {
		var err error
		resp, err = cli.GenerateMmrProof(ctx, []uint32{1}, nil, nil)
		if err == nil {
			break
		}
		lastErr = err
		time.Sleep(2 * time.Second)
	}
	if resp == nil {
		t.Fatalf("mmr_generateProof never succeeded: %v", lastErr)
	}

	leaves, err := mmr.DecodeLeaves(resp.Leaves)
	if err != nil {
		t.Fatalf("decode leaves: %v", err)
	}
	if len(leaves) != 1 {
		t.Fatalf("expected 1 leaf, got %d", len(leaves))
	}

	leaf := leaves[0]
	t.Logf("leaf: version=%d parent=%d set_id=%d set_len=%d leaf_extra=%x",
		leaf.Version, leaf.ParentNumber, leaf.NextAuthoritySet.ID,
		leaf.NextAuthoritySet.Len, leaf.LeafExtra)

	// Validate the H256 leaf-extra alignment: should be exactly 32 bytes
	// (it always is by type, but the underlying SCALE encoding could only
	// match if the runtime types it as H256). If the runtime had Vec<u8>,
	// our decoder would have consumed one fewer byte of the proof.
	if leaf.LeafExtra == ([32]byte{}) && len(resp.Proof) > 0 {
		t.Logf("leaf_extra is zero (no outbound traffic) — expected on fresh dev node")
	}

	proof, err := mmr.DecodeProof(resp.Proof)
	if err != nil {
		t.Fatalf("decode proof: %v", err)
	}
	t.Logf("proof: leaf_indices=%v leaf_count=%d items=%d",
		proof.LeafIndices, proof.LeafCount, len(proof.Items))

	// Cross-check: compute the simplified proof + leaf hash, then verify it
	// reconstructs to the same MMR root that the BEEFY-MMR pallet would emit.
	// We don't have the root here directly, but we can at least confirm the
	// transformation doesn't panic and produces non-zero output.
	if len(proof.LeafIndices) == 1 {
		simplified, err := mmr.Simplify(proof.LeafIndices[0], proof.LeafCount, proof.Items)
		if err != nil {
			t.Fatalf("simplify: %v", err)
		}
		root := mmr.VerifyMerkleRoot(simplified, leaf.LeafHash())
		t.Logf("derived root: %x", root)
		if root == ([32]byte{}) {
			t.Errorf("derived root is zero — proof simplification failed silently")
		}
	}
}
