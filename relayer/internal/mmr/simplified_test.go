package mmr

import (
	"testing"

	"golang.org/x/crypto/sha3"
)

// Build a small reference MMR by hand and check Simplify + VerifyMerkleRoot
// round-trip back to the same root.
//
//	leaves: L0, L1, L2 (leaf_count=3, mmr_size=4)
//
//	pos 0: L0_hash         (height 0)
//	pos 1: L1_hash         (height 0)
//	pos 2: parent(0,1)     (height 1)
//	pos 3: L2_hash         (height 0)  ← also a peak
//
//	root = bag_peaks(pos 2, pos 3) = keccak(pos3 || pos2)
//
//	Per mmr-lib convention, peaks are bagged right-to-left, so the
//	"merkle root" of the MMR equals keccak(right_peak || left_peak).
func TestSimplify_smallMMR(t *testing.T) {
	l0 := keccak([]byte("L0"))
	l1 := keccak([]byte("L1"))
	l2 := keccak([]byte("L2"))

	// height-1 parent over (L0, L1)
	parentL0L1 := keccak2(l0, l1)

	// MMR root: keccak(rightPeak || leftPeak)
	root := keccak2(l2, parentL0L1)

	// substrate-generated proof for leaf index 0:
	//   items = [L1_hash, L2_hash]  (sibling at h=0, then right-bagged peak)
	proofItems := [][32]byte{l1, l2}

	simplified, err := Simplify(0 /*leafIndex*/, 3 /*leafCount*/, proofItems)
	if err != nil {
		t.Fatalf("simplify: %v", err)
	}
	if len(simplified.Items) != 2 {
		t.Fatalf("expected 2 items, got %d", len(simplified.Items))
	}

	got := VerifyMerkleRoot(simplified, l0)
	if got != root {
		t.Errorf("root mismatch\n  got:  %x\n  want: %x", got, root)
	}
}

// Proof for the right-most leaf — no right peak, but one left peak to bag in.
func TestSimplify_smallMMR_rightLeaf(t *testing.T) {
	l0 := keccak([]byte("L0"))
	l1 := keccak([]byte("L1"))
	l2 := keccak([]byte("L2"))

	parentL0L1 := keccak2(l0, l1)
	root := keccak2(l2, parentL0L1)

	// substrate-generated proof for leaf 2 (rightmost): items = [parent(L0,L1)]
	proofItems := [][32]byte{parentL0L1}

	simplified, err := Simplify(2, 3, proofItems)
	if err != nil {
		t.Fatalf("simplify: %v", err)
	}
	if len(simplified.Items) != 1 {
		t.Fatalf("expected 1 item, got %d", len(simplified.Items))
	}

	got := VerifyMerkleRoot(simplified, l2)
	if got != root {
		t.Errorf("root mismatch\n  got:  %x\n  want: %x", got, root)
	}
}

// Position arithmetic spot-checks against the known mmr-lib values.
func TestPositionArithmetic(t *testing.T) {
	cases := []struct {
		index uint64
		pos   uint64
	}{
		{0, 0},   // first leaf
		{1, 1},
		{2, 3},   // after the height-1 join at pos 2
		{3, 4},
		{4, 7},   // after the height-2 join at pos 6
	}
	for _, c := range cases {
		got := leafIndexToPosition(c.index)
		if got != c.pos {
			t.Errorf("leafIndexToPosition(%d): got %d, want %d", c.index, got, c.pos)
		}
	}

	if leafCountToMMRSize(3) != 4 {
		t.Errorf("leafCountToMMRSize(3): want 4")
	}
	if leafCountToMMRSize(4) != 7 {
		t.Errorf("leafCountToMMRSize(4): want 7")
	}
}

func keccak(b []byte) [32]byte {
	h := sha3.NewLegacyKeccak256()
	h.Write(b)
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
