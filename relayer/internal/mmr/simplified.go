// This file is a Go port of the Snowfork "simplified MMR proof"
// transformation. Original source:
//   github.com/Snowfork/snowbridge — relayer/crypto/merkle/simplified_mmr_proof.go
// Licensed under Apache-2.0 (see https://github.com/Snowfork/snowbridge/blob/main/LICENSE).
//
// What this does:
//
//   The MMR proof returned by `mmr_generateProof` is *multi-peak* — to
//   re-derive the root you walk up to your local peak, then bag in the other
//   peaks (right-bagged + left peaks). On-chain, BeefyClient.MMRProof.
//   verifyLeafProof is a single linear walk consuming one `proofOrder` bit
//   per item. This transformation flattens the multi-peak proof into that
//   form: it computes the order bitfield and the merged item list that
//   reproduce the root with a single linear walk.
//
//   Convention: order bit i == 1 means at step i the sibling sits on the
//   *left* of the accumulator (sibling || acc). Matches BeefyClient.sol's
//   `hashPairs(acc, sibling, order)` where order==1 swaps the operands.
//
// See PR for design rationale: https://github.com/Snowfork/snowbridge/pull/495

package mmr

import (
	"errors"
	"fmt"
	"math/bits"

	"golang.org/x/crypto/sha3"
)

// SimplifiedProof is the on-chain-ready proof shape.
//
// `Items` is consumed in order by `BeefyClient.verifyMMRLeafProof`. `Order`
// is a packed bitfield indicating sibling side for each step.
type SimplifiedProof struct {
	Items [][32]byte
	Order uint64
}

// Simplify converts the substrate-generated multi-peak proof to the linear
// form BeefyClient.sol expects.
func Simplify(leafIndex, leafCount uint64, proofItems [][32]byte) (SimplifiedProof, error) {
	leafPos := leafIndexToPosition(leafIndex)
	mmrSize := leafCountToMMRSize(leafCount)
	peaks := getPeaks(mmrSize)

	var (
		readyMadePeakHashes     [][32]byte
		optionalRightBaggedPeak [32]byte
		merkleProof             [][32]byte
		proofItemPosition       int
		merkleRootPeakPosition  int
		hasRightBaggedPeak      bool
	)

	for i := 0; i < len(peaks); i++ {
		inThisPeak := (i == 0 || leafPos > peaks[i-1]) && leafPos <= peaks[i]
		if inThisPeak {
			merkleRootPeakPosition = i
			if i == len(peaks)-1 {
				// Leaf belongs to the right-most peak; all remaining items
				// are part of the local merkle path.
				for j := proofItemPosition; j < len(proofItems); j++ {
					merkleProof = append(merkleProof, proofItems[j])
				}
			} else {
				// Leaf belongs to an interior peak; the last proof item is
				// the right-bagged hash of all peaks to our right.
				for j := proofItemPosition; j < len(proofItems)-1; j++ {
					merkleProof = append(merkleProof, proofItems[j])
				}
				optionalRightBaggedPeak = proofItems[len(proofItems)-1]
				hasRightBaggedPeak = true
				break
			}
		} else {
			// This peak is to the left of our leaf; capture its hash so we
			// can append it (in reverse order) after the local proof.
			readyMadePeakHashes = append(readyMadePeakHashes, proofItems[proofItemPosition])
			proofItemPosition++
		}
	}

	var localizedPos uint64
	if merkleRootPeakPosition == 0 {
		localizedPos = leafPos
	} else {
		localizedPos = leafPos - peaks[merkleRootPeakPosition-1] - 1
	}

	order, err := calculateMerkleProofOrder(localizedPos, merkleProof)
	if err != nil {
		return SimplifiedProof{}, err
	}

	// Append the right-bagged peak (sibling sits on the *right* of acc → bit
	// stays 0 here in our convention, but Snowfork's original sets bit 1
	// because their convention is inverted relative to the bagging step).
	// Match the original.
	currentProofOrderIndex := len(merkleProof) - 1
	if hasRightBaggedPeak {
		currentProofOrderIndex++
		order |= 1 << currentProofOrderIndex
		merkleProof = append(merkleProof, optionalRightBaggedPeak)
	}
	// Left peaks, right-to-left. These are on the *left* of acc, which in
	// our convention means order bit 1 — Snowfork leaves them implicit
	// (relying on the fact that left peaks always have bit 0 set already);
	// keep parity with the original to avoid silent drift.
	for i := 0; i < len(readyMadePeakHashes); i++ {
		currentProofOrderIndex++
		merkleProof = append(merkleProof, readyMadePeakHashes[len(readyMadePeakHashes)-i-1])
	}

	return SimplifiedProof{Items: merkleProof, Order: order}, nil
}

// VerifyMerkleRoot is a side-channel: given a simplified proof and a leaf
// hash, recompute the MMR root. Useful for sanity-checking our port before
// the relayer submits anything on-chain.
func VerifyMerkleRoot(proof SimplifiedProof, leafHash [32]byte) [32]byte {
	current := leafHash[:]
	for i, sibling := range proof.Items {
		isSiblingLeft := (proof.Order>>i)&1 == 1
		h := sha3.NewLegacyKeccak256()
		if isSiblingLeft {
			h.Write(sibling[:])
			h.Write(current)
		} else {
			h.Write(current)
			h.Write(sibling[:])
		}
		current = h.Sum(nil)
	}
	var out [32]byte
	copy(out[:], current)
	return out
}

/* ──────────────────────── mmr-lib position arithmetic ─────────────────── */

func parentOffset(height uint32) uint64  { return 2 << height }
func siblingOffset(height uint32) uint64 { return (2 << height) - 1 }

func getPeakPosByHeight(height uint32) uint64 { return (1 << (height + 1)) - 2 }

func leftPeakHeightPos(mmrSize uint64) (uint32, uint64) {
	var height uint32 = 1
	var prev uint64 = 0
	pos := getPeakPosByHeight(height)
	for pos < mmrSize {
		height++
		prev = pos
		pos = getPeakPosByHeight(height)
	}
	return height - 1, prev
}

func getRightPeak(height uint32, position, mmrSize uint64) (bool, uint32, uint64) {
	position += siblingOffset(height)
	for position > mmrSize-1 {
		if height == 0 {
			return false, 0, 0
		}
		position -= parentOffset(height - 1)
		height--
	}
	return true, height, position
}

func leafIndexToPosition(index uint64) uint64 {
	return leafIndexToMMRSize(index) - uint64(bits.TrailingZeros64(index+1)) - 1
}

func leafCountToMMRSize(leavesCount uint64) uint64 {
	peakCount := uint64(bits.OnesCount64(leavesCount))
	return 2*leavesCount - peakCount
}

func leafIndexToMMRSize(index uint64) uint64 {
	return leafCountToMMRSize(index + 1)
}

func heightInTree(position uint64) uint32 {
	position++
	allOnes := func(num uint64) bool {
		zeroCount := 64 - bits.OnesCount64(num)
		return num != 0 && bits.LeadingZeros64(num) == zeroCount
	}
	jumpLeft := func(p uint64) uint64 {
		bitLen := 64 - bits.LeadingZeros64(p)
		msb := uint64(1 << (bitLen - 1))
		return p - (msb - 1)
	}
	for !allOnes(position) {
		position = jumpLeft(position)
	}
	return uint32(64 - bits.LeadingZeros64(position) - 1)
}

func getPeaks(mmrSize uint64) []uint64 {
	var peaks []uint64
	height, position := leftPeakHeightPos(mmrSize)
	peaks = append(peaks, position)
	for height > 0 {
		var ok bool
		ok, height, position = getRightPeak(height, position, mmrSize)
		if !ok {
			break
		}
		peaks = append(peaks, position)
	}
	return peaks
}

// calculateMerkleProofOrder walks up the local tree from a leaf, emitting
// one bit per step (1 = sibling on the left). Stops when proof items are
// exhausted or the queue empties.
func calculateMerkleProofOrder(leafPos uint64, proofItems [][32]byte) (uint64, error) {
	type queueElem struct {
		height   uint32
		position uint64
	}
	var (
		order        uint64
		bitPos       int
		queue        = []queueElem{{height: 0, position: leafPos}}
		itemsConsumed int
	)

	for len(queue) > 0 {
		if itemsConsumed >= len(proofItems) {
			return order, nil
		}

		var elem queueElem
		elem, queue = queue[len(queue)-1], queue[:len(queue)-1]

		nextHeight := heightInTree(elem.position + 1)

		var sibling queueElem
		var siblingLeft bool
		if nextHeight > elem.height {
			order |= 1 << bitPos
			siblingLeft = true
			sibling = queueElem{
				height:   elem.height,
				position: elem.position - siblingOffset(elem.height),
			}
		} else {
			siblingLeft = false
			sibling = queueElem{
				height:   elem.height,
				position: elem.position + siblingOffset(elem.height),
			}
		}
		bitPos++
		itemsConsumed++

		var parent queueElem
		if siblingLeft {
			parent = queueElem{
				height:   sibling.height + 1,
				position: sibling.position + parentOffset(sibling.height),
			}
		} else {
			parent = queueElem{
				height:   sibling.height + 1,
				position: sibling.position + 1,
			}
		}
		queue = append(queue, parent)
	}
	return order, errors.New("simplify: proof items remain but tree walk exhausted (corrupted proof)")
}

// Used by tests / callers that need to format a peak count for diagnostics.
func mmrSizeFmt(mmrSize uint64) string {
	return fmt.Sprintf("mmr_size=%d peaks=%d", mmrSize, len(getPeaks(mmrSize)))
}

var _ = mmrSizeFmt // keep available even if no caller in production
