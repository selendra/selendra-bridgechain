// Package bitfield is a Go mirror of `contracts/src/utils/Bitfield.sol`.
//
// `BeefyClient.submitInitial` (and the BEEFY commit-reveal flow generally)
// takes `uint256[]` bitfields where bit `i` lives in element `i / 256` at
// position `i % 256` (LSB-first within each word). This package builds
// and inspects those bitfields server-side without round-tripping through
// the contract's `createInitialBitfield` view call.
//
// The contract enforces:
//
//   - `bitfield.length == containerLength(validatorSetLen)`
//   - all bits at index >= validatorSetLen are zero (padding check)
//
// Both invariants are maintained by these helpers as long as you pass
// the same `length` consistently.
package bitfield

import (
	"fmt"
	"math/big"
)

// ContainerLength returns the number of uint256 words needed to hold
// `length` bits — `ceil(length / 256)`. Mirrors `Bitfield.containerLength`
// in Solidity.
func ContainerLength(length int) int {
	if length <= 0 {
		return 0
	}
	return (length + 255) / 256
}

// New returns a zeroed bitfield with capacity for `length` bits.
func New(length int) []*big.Int {
	n := ContainerLength(length)
	out := make([]*big.Int, n)
	for i := range out {
		out[i] = new(big.Int)
	}
	return out
}

// Set turns bit `index` on. Panics if `bf` was not allocated wide enough
// to hold it — callers should always pass a bitfield from `New(length)`
// with `length > index`.
func Set(bf []*big.Int, index int) {
	word, bit := index>>8, uint(index&0xff)
	bf[word].SetBit(bf[word], int(bit), 1)
}

// IsSet returns true iff bit `index` is set. Indices outside the
// bitfield's range return false.
func IsSet(bf []*big.Int, index int) bool {
	word, bit := index>>8, uint(index&0xff)
	if word >= len(bf) {
		return false
	}
	return bf[word].Bit(int(bit)) == 1
}

// From builds a bitfield of capacity `length` with `indices` set. Returns
// an error if any index is out of range — this is the most common
// programmer error and a silent overflow would only surface as an
// `InvalidBitfieldPadding` revert on chain.
func From(indices []int, length int) ([]*big.Int, error) {
	bf := New(length)
	for _, i := range indices {
		if i < 0 || i >= length {
			return nil, fmt.Errorf("bitfield: index %d out of range [0, %d)", i, length)
		}
		Set(bf, i)
	}
	return bf, nil
}

// Indices returns the indices of all set bits in ascending order.
// Useful for processing the `createFinalBitfield` response (which is the
// sampled subset the relayer must reveal signatures for).
func Indices(bf []*big.Int) []int {
	var out []int
	for word, w := range bf {
		for bit := 0; bit < 256; bit++ {
			if w.Bit(bit) == 1 {
				out = append(out, word*256+bit)
			}
		}
	}
	return out
}

// Count returns the population count (number of set bits).
func Count(bf []*big.Int) int {
	n := 0
	for _, w := range bf {
		n += popcount(w)
	}
	return n
}

func popcount(x *big.Int) int {
	n := 0
	for _, w := range x.Bits() {
		// big.Word is uintptr-sized (32 or 64). Use the standard
		// shift-and-and popcount — it's not the hot path.
		v := uint64(w)
		v = v - ((v >> 1) & 0x5555555555555555)
		v = (v & 0x3333333333333333) + ((v >> 2) & 0x3333333333333333)
		v = (v + (v >> 4)) & 0x0f0f0f0f0f0f0f0f
		n += int((v * 0x0101010101010101) >> 56)
	}
	return n
}
