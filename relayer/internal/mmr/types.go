// Package mmr models the bridgechain BEEFY-MMR leaf and the proof shape the
// node's `mmr_generateProof` RPC returns.
//
// Wire-format reference (in `substrate/primitives/consensus/beefy/`):
//
//	MmrLeaf {
//	    version: u8,
//	    parent_number_and_hash: (BlockNumber, Hash),  // (u32, H256)
//	    beefy_next_authority_set: BeefyAuthoritySet { id: u64, len: u32, root: H256 },
//	    leaf_extra: H256,                              // bridgechain-specific: flat 32 bytes
//	}
//
// SCALE-encoded leaf is therefore a fixed 1 + 4 + 32 + 8 + 4 + 32 + 32 = 113 bytes.
package mmr

import (
	"fmt"

	"github.com/nathselendra/bridgechain/relayer/internal/scale"
	"golang.org/x/crypto/sha3"
)

// AuthoritySet is `BeefyAuthoritySet<MerkleRoot>` from sp_consensus_beefy::mmr.
type AuthoritySet struct {
	ID   uint64
	Len  uint32
	Root [32]byte
}

// Leaf is the bridgechain BEEFY-MMR leaf shape (with H256 leaf_extra).
type Leaf struct {
	Version           uint8
	ParentNumber      uint32
	ParentHash        [32]byte
	NextAuthoritySet  AuthoritySet
	LeafExtra         [32]byte
}

// Proof is the merkle-mountain-range proof returned by `mmr_generateProof`.
// `LeafIndices` is one-element for single-leaf proofs.
type Proof struct {
	LeafIndices []uint64
	LeafCount   uint64
	Items       [][32]byte
}

// DecodeLeaf reads a single SCALE-encoded Leaf from r.
func DecodeLeaf(r *scale.Reader) (*Leaf, error) {
	var leaf Leaf
	v, err := r.U8()
	if err != nil {
		return nil, fmt.Errorf("leaf version: %w", err)
	}
	leaf.Version = v

	pn, err := r.U32()
	if err != nil {
		return nil, fmt.Errorf("leaf parent_number: %w", err)
	}
	leaf.ParentNumber = pn

	if _, err := r.Read(leaf.ParentHash[:]); err != nil {
		return nil, fmt.Errorf("leaf parent_hash: %w", err)
	}

	id, err := r.U64()
	if err != nil {
		return nil, fmt.Errorf("leaf next_set.id: %w", err)
	}
	leaf.NextAuthoritySet.ID = id

	length, err := r.U32()
	if err != nil {
		return nil, fmt.Errorf("leaf next_set.len: %w", err)
	}
	leaf.NextAuthoritySet.Len = length

	if _, err := r.Read(leaf.NextAuthoritySet.Root[:]); err != nil {
		return nil, fmt.Errorf("leaf next_set.root: %w", err)
	}

	if _, err := r.Read(leaf.LeafExtra[:]); err != nil {
		return nil, fmt.Errorf("leaf leaf_extra: %w", err)
	}
	return &leaf, nil
}

// DecodeLeaves reads `Vec<Leaf>` (compact length-prefixed). Used for the
// leaves field of `mmr_generateProof` responses.
func DecodeLeaves(buf []byte) ([]Leaf, error) {
	r := scale.NewReader(buf)
	n, err := r.Compact()
	if err != nil {
		return nil, fmt.Errorf("leaves length: %w", err)
	}
	out := make([]Leaf, 0, n)
	for i := uint64(0); i < n; i++ {
		leaf, err := DecodeLeaf(r)
		if err != nil {
			return nil, fmt.Errorf("leaves[%d]: %w", i, err)
		}
		out = append(out, *leaf)
	}
	return out, nil
}

// DecodeProof reads a SCALE-encoded `mmr_lib::Proof`.
func DecodeProof(buf []byte) (*Proof, error) {
	r := scale.NewReader(buf)

	n, err := r.Compact()
	if err != nil {
		return nil, fmt.Errorf("proof leaf_indices length: %w", err)
	}
	indices := make([]uint64, n)
	for i := uint64(0); i < n; i++ {
		v, err := r.U64()
		if err != nil {
			return nil, fmt.Errorf("proof leaf_indices[%d]: %w", i, err)
		}
		indices[i] = v
	}

	leafCount, err := r.U64()
	if err != nil {
		return nil, fmt.Errorf("proof leaf_count: %w", err)
	}

	m, err := r.Compact()
	if err != nil {
		return nil, fmt.Errorf("proof items length: %w", err)
	}
	items := make([][32]byte, m)
	for i := uint64(0); i < m; i++ {
		if _, err := r.Read(items[i][:]); err != nil {
			return nil, fmt.Errorf("proof items[%d]: %w", i, err)
		}
	}

	return &Proof{LeafIndices: indices, LeafCount: leafCount, Items: items}, nil
}

// LeafHash returns keccak256 of the SCALE encoding of the leaf — the value
// `BeefyClient.verifyMMRLeafProof` expects as its `leafHash` argument and
// the input the Solidity `Gateway.hashMmrLeaf` produces.
func (l *Leaf) LeafHash() [32]byte {
	h := sha3.NewLegacyKeccak256()
	h.Write([]byte{l.Version})
	var u32 [4]byte
	leUint32(u32[:], l.ParentNumber)
	h.Write(u32[:])
	h.Write(l.ParentHash[:])
	var u64 [8]byte
	leUint64(u64[:], l.NextAuthoritySet.ID)
	h.Write(u64[:])
	leUint32(u32[:], l.NextAuthoritySet.Len)
	h.Write(u32[:])
	h.Write(l.NextAuthoritySet.Root[:])
	h.Write(l.LeafExtra[:])
	var out [32]byte
	copy(out[:], h.Sum(nil))
	return out
}

// TODO: proof-order computation.
//
// `MMRProof.verifyLeafProof` in BeefyClient.sol takes a `proofOrder` bitfield
// describing, at each step, whether the proof item sits on the left or the
// right of the accumulator. mmr-lib (Rust) derives it from `leaf_index` +
// `leaf_count` via `gen_proof_positions`. Porting that algorithm here is
// straightforward but easy to subtly mis-implement — wrong order bits will
// produce a wrong root, which the contract rejects as `InvalidMmrLeafProof`.
//
// Plan: cover this with a runtime-API helper on the Substrate side
// (`BridgeOutboundApi::mmr_proof_order(leaf_index, leaf_count) -> u256`) so
// both ends share the same Rust implementation. Until then, treat
// `MmrProofOrder` as not-yet-implemented; the relayer can decode the proof
// structure but cannot submit it for verification.

func leUint32(dst []byte, v uint32) {
	dst[0] = byte(v)
	dst[1] = byte(v >> 8)
	dst[2] = byte(v >> 16)
	dst[3] = byte(v >> 24)
}

func leUint64(dst []byte, v uint64) {
	dst[0] = byte(v)
	dst[1] = byte(v >> 8)
	dst[2] = byte(v >> 16)
	dst[3] = byte(v >> 24)
	dst[4] = byte(v >> 32)
	dst[5] = byte(v >> 40)
	dst[6] = byte(v >> 48)
	dst[7] = byte(v >> 56)
}
