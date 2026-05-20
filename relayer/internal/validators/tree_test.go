package validators

import (
	"testing"
)

// verifyProof mirrors `SubstrateMerkleProof.computeRoot` in
// contracts/src/utils/SubstrateMerkleProof.sol — used here to confirm our
// Go-side `Tree.Proof` produces siblings that round-trip through the same
// algorithm the on-chain verifier runs.
func verifyProof(leaf [32]byte, position, width int, proof [][32]byte) [32]byte {
	node := leaf
	for _, sibling := range proof {
		if position&1 == 1 || position+1 == width {
			node = keccak2(sibling, node)
		} else {
			node = keccak2(node, sibling)
		}
		position >>= 1
		width = ((width - 1) >> 1) + 1
	}
	return node
}

func addr(b byte) [20]byte {
	var a [20]byte
	for i := range a {
		a[i] = b
	}
	return a
}

func TestTree_singleValidator(t *testing.T) {
	tree, err := New([][20]byte{addr(0x01)})
	if err != nil {
		t.Fatal(err)
	}
	if tree.Root() != keccakAddr(addr(0x01)) {
		t.Errorf("root: expected leaf hash for single-leaf tree")
	}
	proof, err := tree.Proof(0)
	if err != nil {
		t.Fatal(err)
	}
	if len(proof) != 0 {
		t.Errorf("single-leaf proof should be empty, got %d items", len(proof))
	}
}

func TestTree_twoValidators(t *testing.T) {
	tree, err := New([][20]byte{addr(0x01), addr(0x02)})
	if err != nil {
		t.Fatal(err)
	}
	l0, l1 := keccakAddr(addr(0x01)), keccakAddr(addr(0x02))
	if tree.Root() != keccak2(l0, l1) {
		t.Errorf("root mismatch")
	}
	for i := 0; i < 2; i++ {
		proof, err := tree.Proof(i)
		if err != nil {
			t.Fatal(err)
		}
		got := verifyProof(tree.leaves[i], i, 2, proof)
		if got != tree.Root() {
			t.Errorf("leaf %d: verify mismatch", i)
		}
	}
}

func TestTree_oddValidators(t *testing.T) {
	// 5 validators exercises odd-one-out promotion at multiple levels.
	addrs := [][20]byte{addr(0x01), addr(0x02), addr(0x03), addr(0x04), addr(0x05)}
	tree, err := New(addrs)
	if err != nil {
		t.Fatal(err)
	}

	for i := 0; i < len(addrs); i++ {
		proof, err := tree.Proof(i)
		if err != nil {
			t.Fatalf("proof %d: %v", i, err)
		}
		got := verifyProof(tree.leaves[i], i, len(addrs), proof)
		if got != tree.Root() {
			t.Errorf("leaf %d: verify mismatch", i)
		}
	}
}

func TestTree_powerOfTwo(t *testing.T) {
	addrs := make([][20]byte, 8)
	for i := range addrs {
		addrs[i] = addr(byte(i + 1))
	}
	tree, err := New(addrs)
	if err != nil {
		t.Fatal(err)
	}
	for i := 0; i < len(addrs); i++ {
		proof, err := tree.Proof(i)
		if err != nil {
			t.Fatalf("proof %d: %v", i, err)
		}
		if len(proof) != 3 {
			t.Errorf("leaf %d: expected proof length 3, got %d", i, len(proof))
		}
		got := verifyProof(tree.leaves[i], i, len(addrs), proof)
		if got != tree.Root() {
			t.Errorf("leaf %d: verify mismatch", i)
		}
	}
}

func TestTree_emptyRejected(t *testing.T) {
	if _, err := New(nil); err == nil {
		t.Errorf("expected error for empty set")
	}
}

func TestTree_outOfRange(t *testing.T) {
	tree, _ := New([][20]byte{addr(0x01)})
	if _, err := tree.Proof(5); err == nil {
		t.Errorf("expected error for out-of-range index")
	}
}
