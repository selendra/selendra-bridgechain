package driver

import (
	"testing"

	"github.com/nathselendra/bridgechain/relayer/internal/bitfield"
	"github.com/nathselendra/bridgechain/relayer/internal/mmr"
	"github.com/nathselendra/bridgechain/relayer/internal/proof"
	"github.com/nathselendra/bridgechain/relayer/internal/validators"
)

func sampleLeaf() mmr.Leaf {
	var parent, setRoot, leafExtra [32]byte
	for i := range parent {
		parent[i] = 0x11
		setRoot[i] = 0x22
		leafExtra[i] = 0x33
	}
	return mmr.Leaf{
		Version:      0,
		ParentNumber: 9,
		ParentHash:   parent,
		NextAuthoritySet: mmr.AuthoritySet{
			ID:   2,
			Len:  4,
			Root: setRoot,
		},
		LeafExtra: leafExtra,
	}
}

func TestBuildFinalSubmission_threeOfFour(t *testing.T) {
	// 4 validators, all signed. Contract samples indices [0, 1, 3] in
	// the reveal phase.
	commit, addrs := buildSignedCommitment(t, []int{0, 1, 2, 3})
	tree, err := validators.New(addrs)
	if err != nil {
		t.Fatalf("tree: %v", err)
	}

	leafProof := [][32]byte{
		{0xa1, 0xa2}, // dummy proof item 0
		{0xb1, 0xb2}, // dummy proof item 1
	}
	const leafProofOrder = uint64(0b01)
	finalIndices := []int{0, 1, 3}

	fs, err := BuildFinalSubmission(commit, tree, finalIndices,
		sampleLeaf(), leafProof, leafProofOrder)
	if err != nil {
		t.Fatalf("build: %v", err)
	}

	// Bitfield has exactly the sampled indices set.
	if got := bitfield.Indices(fs.Bitfield); !equalInts(got, finalIndices) {
		t.Errorf("bitfield: got %v, want %v", got, finalIndices)
	}

	// One ValidatorProof per index, in ascending order.
	if len(fs.Proofs) != len(finalIndices) {
		t.Fatalf("proof count: got %d, want %d", len(fs.Proofs), len(finalIndices))
	}
	for i, idx := range finalIndices {
		if fs.Proofs[i].Index.Int64() != int64(idx) {
			t.Errorf("proof[%d].index: got %s, want %d", i,
				fs.Proofs[i].Index.String(), idx)
		}
		if [20]byte(fs.Proofs[i].Account) != addrs[idx] {
			t.Errorf("proof[%d].account: got %x, want %x", i,
				fs.Proofs[i].Account, addrs[idx])
		}
	}

	// Leaf fields survive the conversion. The abigen struct's
	// `ParachainHeadsRoot` is our `LeafExtra` — same 32 bytes.
	if fs.Leaf.ParentNumber != 9 || fs.Leaf.NextAuthoritySetID != 2 {
		t.Errorf("leaf fields: %+v", fs.Leaf)
	}
	if fs.Leaf.ParachainHeadsRoot != sampleLeaf().LeafExtra {
		t.Errorf("leaf_extra mapping mismatch")
	}

	if fs.LeafProofOrder.Uint64() != leafProofOrder {
		t.Errorf("proof order: got %s, want %d",
			fs.LeafProofOrder.String(), leafProofOrder)
	}

	// CommitmentHash matches what proof.CommitmentHash returns.
	want := proof.CommitmentHash(commit.Commitment)
	if fs.CommitmentHash != want {
		t.Errorf("commitment hash mismatch")
	}
}

func TestBuildFinalSubmission_rejectsAbsentSignature(t *testing.T) {
	// Only validator 0 signed; sampling validator 1 should fail.
	commit, addrs := buildSignedCommitment(t, []int{0})
	tree, _ := validators.New(addrs)

	_, err := BuildFinalSubmission(commit, tree, []int{1},
		sampleLeaf(), nil, 0)
	if err == nil {
		t.Fatal("expected error when sampling absent signer")
	}
}

func TestBuildFinalSubmission_rejectsEmptyIndices(t *testing.T) {
	commit, addrs := buildSignedCommitment(t, []int{0})
	tree, _ := validators.New(addrs)

	_, err := BuildFinalSubmission(commit, tree, nil,
		sampleLeaf(), nil, 0)
	if err == nil {
		t.Fatal("expected error on empty final indices")
	}
}

func TestBuildFinalSubmission_rejectsOutOfRange(t *testing.T) {
	commit, addrs := buildSignedCommitment(t, []int{0, 1, 2, 3})
	tree, _ := validators.New(addrs)

	_, err := BuildFinalSubmission(commit, tree, []int{0, 99},
		sampleLeaf(), nil, 0)
	if err == nil {
		t.Fatal("expected error when index >= validator-set length")
	}
}

func equalInts(a, b []int) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
