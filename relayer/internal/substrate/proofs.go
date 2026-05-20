package substrate

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"
)

// LeavesProof is the JSON shape returned by `mmr_generateProof`.
//
// `Leaves` and `Proof` are SCALE-encoded byte blobs serialized as "0x..."
// hex strings on the wire. Decode them with the `internal/mmr` package.
type LeavesProof struct {
	BlockHash [32]byte
	Leaves    []byte // SCALE: Vec<MmrLeaf>
	Proof     []byte // SCALE: mmr_lib::Proof<MmrHash>
}

// GenerateMmrProof calls `mmr_generateProof(blockNumbers, bestKnownBlock, at)`.
// Pass `nil` for `bestKnownBlock` to use the node's best-known block; pass an
// empty `at` (zero value) to query at the latest block.
func (c *Client) GenerateMmrProof(ctx context.Context, blockNumbers []uint32,
	bestKnownBlock *uint32, at *[32]byte) (*LeavesProof, error) {
	args := []any{blockNumbers}
	if bestKnownBlock != nil {
		args = append(args, *bestKnownBlock)
	} else {
		args = append(args, nil)
	}
	if at != nil {
		args = append(args, hexEncode(at[:]))
	} else {
		args = append(args, nil)
	}
	raw, err := c.Call(ctx, "mmr_generateProof", args...)
	if err != nil {
		return nil, fmt.Errorf("substrate: mmr_generateProof: %w", err)
	}
	var payload struct {
		BlockHash string `json:"blockHash"`
		Leaves    string `json:"leaves"`
		Proof     string `json:"proof"`
	}
	if err := json.Unmarshal(raw, &payload); err != nil {
		return nil, fmt.Errorf("substrate: mmr_generateProof: decode response: %w", err)
	}

	bh, err := hexDecodeArray32(payload.BlockHash)
	if err != nil {
		return nil, fmt.Errorf("substrate: mmr_generateProof: blockHash: %w", err)
	}
	leaves, err := hexDecode(payload.Leaves)
	if err != nil {
		return nil, fmt.Errorf("substrate: mmr_generateProof: leaves: %w", err)
	}
	proof, err := hexDecode(payload.Proof)
	if err != nil {
		return nil, fmt.Errorf("substrate: mmr_generateProof: proof: %w", err)
	}
	return &LeavesProof{BlockHash: bh, Leaves: leaves, Proof: proof}, nil
}

// StateCall invokes a runtime API via the `state_call` JSON-RPC method.
//
// `method` is the colon-prefixed API path, e.g.
// "BridgeOutboundApi_message_proof". `data` is the SCALE-encoded argument
// tuple. Pass `at` = nil for the latest block.
//
// Returns the raw SCALE-encoded return value.
func (c *Client) StateCall(ctx context.Context, method string, data []byte, at *[32]byte) ([]byte, error) {
	args := []any{method, hexEncode(data)}
	if at != nil {
		args = append(args, hexEncode(at[:]))
	} else {
		args = append(args, nil)
	}
	raw, err := c.Call(ctx, "state_call", args...)
	if err != nil {
		return nil, fmt.Errorf("substrate: state_call %s: %w", method, err)
	}
	var s string
	if err := json.Unmarshal(raw, &s); err != nil {
		return nil, fmt.Errorf("substrate: state_call %s: decode: %w", method, err)
	}
	return hexDecode(s)
}

// hexEncode formats a byte slice as a 0x-prefixed lowercase hex string.
func hexEncode(b []byte) string {
	return "0x" + hex.EncodeToString(b)
}

// hexDecode parses a 0x-prefixed hex string. Empty strings decode to nil.
func hexDecode(s string) ([]byte, error) {
	s = strings.TrimPrefix(s, "0x")
	if s == "" {
		return nil, nil
	}
	return hex.DecodeString(s)
}

func hexDecodeArray32(s string) ([32]byte, error) {
	b, err := hexDecode(s)
	if err != nil {
		return [32]byte{}, err
	}
	if len(b) != 32 {
		return [32]byte{}, fmt.Errorf("expected 32 bytes, got %d", len(b))
	}
	var out [32]byte
	copy(out[:], b)
	return out, nil
}
