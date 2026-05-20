// Package validators builds the BEEFY validator-set Merkle tree the relayer
// needs for `BeefyClient.submitInitial` / `submitFinal`.
//
// The tree shape mirrors substrate's `binary-merkle-tree` crate:
//
//   - each leaf = keccak256(validator_eth_address)  (input is 20 bytes,
//     leaf is 32)
//   - pair-hash: keccak256(left ‖ right)
//   - odd nodes at any level are *promoted* unhashed, not duplicated
//
// Same algorithm as `pallet-bridge-outbound`'s per-block message tree —
// keeping them aligned simplifies the Solidity side (it has a single
// `SubstrateMerkleProof.verify` implementation that handles both).
package validators

import (
	"fmt"

	"golang.org/x/crypto/sha3"
)

// Tree is an immutable validator-set tree. Build it once per validator-set
// rotation; use it to produce inclusion proofs for sampled validators.
type Tree struct {
	leaves [][32]byte
	layers [][][32]byte // layers[0] = hashed leaves, layers[N-1] = [root]
}

// New builds the tree over the supplied 20-byte addresses. Addresses must
// already be in the order BEEFY emits them — the relayer doesn't sort.
func New(addresses [][20]byte) (*Tree, error) {
	if len(addresses) == 0 {
		return nil, fmt.Errorf("validators: empty set")
	}
	leaves := make([][32]byte, len(addresses))
	for i, a := range addresses {
		leaves[i] = keccakAddr(a)
	}

	// Build layers bottom-up. Final layer always has exactly one node.
	layers := [][][32]byte{leaves}
	for len(layers[len(layers)-1]) > 1 {
		prev := layers[len(layers)-1]
		next := make([][32]byte, 0, (len(prev)+1)/2)
		for i := 0; i < len(prev); i += 2 {
			if i+1 < len(prev) {
				next = append(next, keccak2(prev[i], prev[i+1]))
			} else {
				// Odd one out — promote unhashed.
				next = append(next, prev[i])
			}
		}
		layers = append(layers, next)
	}

	return &Tree{leaves: leaves, layers: layers}, nil
}

// Root returns the keccak Merkle root over the validator set.
func (t *Tree) Root() [32]byte {
	return t.layers[len(t.layers)-1][0]
}

// Len returns the number of validators in the set.
func (t *Tree) Len() int { return len(t.leaves) }

// Proof returns the sibling chain for the validator at `index`. The slice
// is ordered leaf-to-root, suitable for `SubstrateMerkleProof.verify` on
// the Solidity side and for `BeefyClient.ValidatorProof.proof`.
func (t *Tree) Proof(index int) ([][32]byte, error) {
	if index < 0 || index >= len(t.leaves) {
		return nil, fmt.Errorf("validators: index %d out of range (%d leaves)", index, len(t.leaves))
	}

	proof := make([][32]byte, 0, len(t.layers)-1)
	pos := index
	for level := 0; level < len(t.layers)-1; level++ {
		layer := t.layers[level]
		width := len(layer)
		if pos&1 == 1 {
			// We're the right child. Sibling is on the left.
			proof = append(proof, layer[pos-1])
		} else if pos+1 < width {
			// We're a left child with an actual right sibling.
			proof = append(proof, layer[pos+1])
		}
		// If pos+1 == width and pos is even, we were odd-one-out at this
		// level — promoted untouched, no sibling, nothing to append.
		pos >>= 1
	}
	return proof, nil
}

func keccakAddr(addr [20]byte) [32]byte {
	h := sha3.NewLegacyKeccak256()
	h.Write(addr[:])
	var out [32]byte
	copy(out[:], h.Sum(nil))
	return out
}

func keccak2(a, b [32]byte) [32]byte {
	h := sha3.NewLegacyKeccak256()
	h.Write(a[:])
	h.Write(b[:])
	var out [32]byte
	copy(out[:], h.Sum(nil))
	return out
}
