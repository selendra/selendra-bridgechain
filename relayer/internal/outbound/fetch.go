// Package outbound fetches the data a relayer needs to submit a single
// substrate→ethereum message to the Gateway contract:
//
//   - the message struct itself (nonce, destination, payload),
//   - the per-block Merkle proof of that message under leaf_extra,
//   - the BEEFY-MMR leaf for that block,
//   - the MMR-leaf inclusion proof against the latest BeefyClient root.
//
// All four come from the bridgechain node — three via RPC, one via the
// `BridgeOutboundApi::message_proof` runtime API.
package outbound

import (
	"bytes"
	"context"
	"encoding/binary"
	"fmt"

	"github.com/nathselendra/bridgechain/relayer/internal/mmr"
	"github.com/nathselendra/bridgechain/relayer/internal/scale"
	"github.com/nathselendra/bridgechain/relayer/internal/substrate"
)

// Message mirrors `pallet_bridge_outbound::OutboundMessage`.
type Message struct {
	Nonce       uint64
	Destination [20]byte
	Payload     []byte
}

// MessageProof mirrors `pallet_bridge_outbound::OutboundMessageProof`.
type MessageProof struct {
	Root       [32]byte
	Items      [][32]byte
	LeafCount  uint32
	LeafIndex  uint32
	LeafBytes  []byte // SCALE(Message) — keccak256 of this is the merkle leaf
}

// Bundle is the full submission payload for one message.
type Bundle struct {
	Message      Message
	MessageProof MessageProof

	// MMR leaf containing the message's commitment in its `leaf_extra`.
	MmrLeaf mmr.Leaf

	// SCALE bytes of the BEEFY-MMR leaf, used to compute `keccak256(leaf)`
	// against the on-chain MMRProof verifier.
	MmrLeafBytes []byte

	// Raw MMR proof of the leaf in the MMR root currently trusted by the
	// BeefyClient. Preserved alongside `MmrProofSimplified` mostly for
	// diagnostics — production callers should use the simplified form.
	MmrProof mmr.Proof

	// Linearized MMR proof — what Gateway.submitInbound consumes. Computed
	// from the raw proof + leaf_index + leaf_count via the Snowfork port at
	// internal/mmr/simplified.go.
	MmrProofSimplified mmr.SimplifiedProof
}

// FetchMessageBundle pulls everything needed to submit message `index` from
// block `block`.
//
// `bestBlock` is the block whose MMR root the relayer plans to verify
// against (typically the latest BEEFY-justified block on the Ethereum side).
// `at` is the Substrate block hash at which to query; nil = latest finalized.
func FetchMessageBundle(ctx context.Context, sub *substrate.Client,
	block uint32, index uint32, bestBlock *uint32, at *[32]byte) (*Bundle, error) {

	// 1. Per-block message Merkle proof from the runtime API.
	args := bytes.NewBuffer(nil)
	args.Write(leUint32(block))
	args.Write(leUint32(index))
	raw, err := sub.StateCall(ctx, "BridgeOutboundApi_message_proof", args.Bytes(), at)
	if err != nil {
		return nil, fmt.Errorf("outbound: state_call: %w", err)
	}
	mp, err := decodeMessageProofOption(raw)
	if err != nil {
		return nil, fmt.Errorf("outbound: decode message_proof: %w", err)
	}
	if mp == nil {
		return nil, fmt.Errorf("outbound: no message at (block=%d, index=%d)", block, index)
	}

	// 2. Message itself, also from the runtime API (cheap second call —
	//    keeps the bundle self-contained without recomputing SCALE on the
	//    relayer side).
	args.Reset()
	args.Write(leUint32(block))
	rawMsgs, err := sub.StateCall(ctx, "BridgeOutboundApi_messages_at", args.Bytes(), at)
	if err != nil {
		return nil, fmt.Errorf("outbound: messages_at: %w", err)
	}
	messages, err := decodeMessages(rawMsgs)
	if err != nil {
		return nil, fmt.Errorf("outbound: decode messages: %w", err)
	}
	if int(index) >= len(messages) {
		return nil, fmt.Errorf("outbound: index %d out of range (got %d messages)", index, len(messages))
	}

	// 3. MMR leaf + leaf proof. `block` is the block the message was
	//    queued in; the BEEFY-MMR leaf describing it is appended during
	//    `on_initialize(block+1)`. We therefore ask for the leaf at
	//    `block+1` and verify against `bestBlock`'s MMR root.
	target := []uint32{block + 1}
	leavesProof, err := sub.GenerateMmrProof(ctx, target, bestBlock, at)
	if err != nil {
		return nil, fmt.Errorf("outbound: mmr_generateProof: %w", err)
	}
	leaves, err := mmr.DecodeLeaves(leavesProof.Leaves)
	if err != nil {
		return nil, fmt.Errorf("outbound: decode leaves: %w", err)
	}
	if len(leaves) != 1 {
		return nil, fmt.Errorf("outbound: expected 1 leaf, got %d", len(leaves))
	}
	proof, err := mmr.DecodeProof(leavesProof.Proof)
	if err != nil {
		return nil, fmt.Errorf("outbound: decode proof: %w", err)
	}

	// `mmr_generateProof` only handles single-leaf proofs for the bridge use
	// case. Anything else means our caller asked for a batch we don't know
	// how to flatten.
	if len(proof.LeafIndices) != 1 {
		return nil, fmt.Errorf("outbound: expected 1 leaf index in MMR proof, got %d", len(proof.LeafIndices))
	}
	simplified, err := mmr.Simplify(proof.LeafIndices[0], proof.LeafCount, proof.Items)
	if err != nil {
		return nil, fmt.Errorf("outbound: simplify MMR proof: %w", err)
	}

	return &Bundle{
		Message:            messages[index],
		MessageProof:       *mp,
		MmrLeaf:            leaves[0],
		MmrLeafBytes:       encodeLeaf(&leaves[0]),
		MmrProof:           *proof,
		MmrProofSimplified: simplified,
	}, nil
}

func decodeMessageProofOption(buf []byte) (*MessageProof, error) {
	r := scale.NewReader(buf)
	tag, err := r.U8()
	if err != nil {
		return nil, fmt.Errorf("option tag: %w", err)
	}
	switch tag {
	case 0:
		return nil, nil
	case 1:
		var root [32]byte
		if _, err := r.Read(root[:]); err != nil {
			return nil, fmt.Errorf("root: %w", err)
		}
		nItems, err := r.Compact()
		if err != nil {
			return nil, fmt.Errorf("proof items length: %w", err)
		}
		items := make([][32]byte, nItems)
		for i := uint64(0); i < nItems; i++ {
			if _, err := r.Read(items[i][:]); err != nil {
				return nil, fmt.Errorf("proof items[%d]: %w", i, err)
			}
		}
		leafCount, err := r.U32()
		if err != nil {
			return nil, fmt.Errorf("leaf_count: %w", err)
		}
		leafIndex, err := r.U32()
		if err != nil {
			return nil, fmt.Errorf("leaf_index: %w", err)
		}
		leaf, err := r.ByteSlice()
		if err != nil {
			return nil, fmt.Errorf("leaf bytes: %w", err)
		}
		return &MessageProof{
			Root:      root,
			Items:     items,
			LeafCount: leafCount,
			LeafIndex: leafIndex,
			LeafBytes: leaf,
		}, nil
	default:
		return nil, fmt.Errorf("unexpected Option tag %d", tag)
	}
}

func decodeMessages(buf []byte) ([]Message, error) {
	r := scale.NewReader(buf)
	n, err := r.Compact()
	if err != nil {
		return nil, fmt.Errorf("vec length: %w", err)
	}
	out := make([]Message, 0, n)
	for i := uint64(0); i < n; i++ {
		var m Message
		if m.Nonce, err = r.U64(); err != nil {
			return nil, fmt.Errorf("messages[%d].nonce: %w", i, err)
		}
		if _, err := r.Read(m.Destination[:]); err != nil {
			return nil, fmt.Errorf("messages[%d].destination: %w", i, err)
		}
		payload, err := r.ByteSlice()
		if err != nil {
			return nil, fmt.Errorf("messages[%d].payload: %w", i, err)
		}
		m.Payload = payload
		out = append(out, m)
	}
	return out, nil
}

func encodeLeaf(l *mmr.Leaf) []byte {
	var buf bytes.Buffer
	buf.WriteByte(l.Version)
	binary.Write(&buf, binary.LittleEndian, l.ParentNumber)
	buf.Write(l.ParentHash[:])
	binary.Write(&buf, binary.LittleEndian, l.NextAuthoritySet.ID)
	binary.Write(&buf, binary.LittleEndian, l.NextAuthoritySet.Len)
	buf.Write(l.NextAuthoritySet.Root[:])
	buf.Write(l.LeafExtra[:])
	return buf.Bytes()
}

func leUint32(v uint32) []byte {
	out := make([]byte, 4)
	binary.LittleEndian.PutUint32(out, v)
	return out
}
