package driver

import (
	"bytes"
	"math/big"
	"testing"

	ethcrypto "github.com/ethereum/go-ethereum/crypto"
	"github.com/nathselendra/bridgechain/relayer/internal/beefy"
	"github.com/nathselendra/bridgechain/relayer/internal/bitfield"
	"github.com/nathselendra/bridgechain/relayer/internal/proof"
	"github.com/nathselendra/bridgechain/relayer/internal/validators"
)

// Anvil's deterministic test keys — same set used in proof_test.go so we
// don't have two unrelated key fixtures floating around.
var anvilKeys = []string{
	"ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
	"59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
	"5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
	"7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
}

// buildSignedCommitment returns a SignedCommitment over a fixed test
// commitment, with signatures from `signerIndices` only — the others are
// nil. Also returns the matching validator address list (always in
// validator-index order, for all keys, signed or not).
func buildSignedCommitment(
	t *testing.T,
	signerIndices []int,
) (*beefy.SignedCommitment, [][20]byte) {
	t.Helper()

	c := beefy.Commitment{
		Payload: []beefy.PayloadItem{{
			ID:   [2]byte{'m', 'h'},
			Data: bytes.Repeat([]byte{0xab}, 32),
		}},
		BlockNumber:    42,
		ValidatorSetID: 7,
	}
	hash := proof.CommitmentHash(c)

	signers := make(map[int]bool, len(signerIndices))
	for _, i := range signerIndices {
		signers[i] = true
	}

	addrs := make([][20]byte, len(anvilKeys))
	sigs := make([]*beefy.Signature, len(anvilKeys))
	for i, hexKey := range anvilKeys {
		key, err := ethcrypto.HexToECDSA(hexKey)
		if err != nil {
			t.Fatalf("hex key %d: %v", i, err)
		}
		copy(addrs[i][:], ethcrypto.PubkeyToAddress(key.PublicKey).Bytes())

		if !signers[i] {
			continue
		}
		sigBytes, err := ethcrypto.Sign(hash[:], key)
		if err != nil {
			t.Fatalf("sign %d: %v", i, err)
		}
		var sig beefy.Signature
		copy(sig[:], sigBytes)
		sigs[i] = &sig
	}
	return &beefy.SignedCommitment{Commitment: c, Signatures: sigs}, addrs
}

func TestBuildInitialSubmission_singleSigner(t *testing.T) {
	commit, addrs := buildSignedCommitment(t, []int{0})
	tree, err := validators.New(addrs)
	if err != nil {
		t.Fatalf("tree: %v", err)
	}

	is, err := BuildInitialSubmission(commit, tree)
	if err != nil {
		t.Fatalf("build: %v", err)
	}

	// CommitmentHash matches the proof package's value for the same input.
	wantHash := proof.CommitmentHash(commit.Commitment)
	if is.CommitmentHash != wantHash {
		t.Errorf("commitment hash mismatch")
	}

	// Bitfield has exactly one bit set — at the signer's index.
	if bitfield.Count(is.Bitfield) != 1 {
		t.Errorf("bitfield count: got %d, want 1", bitfield.Count(is.Bitfield))
	}
	if !bitfield.IsSet(is.Bitfield, 0) {
		t.Errorf("bitfield bit 0: not set")
	}

	// Container length matches the contract's expectation for 4 validators.
	if len(is.Bitfield) != bitfield.ContainerLength(4) {
		t.Errorf("container len: got %d, want %d",
			len(is.Bitfield), bitfield.ContainerLength(4))
	}

	// Seed proof points at the only present signature.
	if is.Proof.Index.Cmp(big.NewInt(0)) != 0 {
		t.Errorf("seed index: got %s, want 0", is.Proof.Index.String())
	}
	if [20]byte(is.Proof.Account) != addrs[0] {
		t.Errorf("seed account: got %x, want %x", is.Proof.Account, addrs[0])
	}
	if is.Proof.V != 27 && is.Proof.V != 28 {
		t.Errorf("seed v: got %d, want 27 or 28", is.Proof.V)
	}

	// Commitment payload survived the conversion.
	if is.Commitment.BlockNumber != 42 || is.Commitment.ValidatorSetID != 7 {
		t.Errorf("commitment fields: %+v", is.Commitment)
	}
	if len(is.Commitment.Payload) != 1 {
		t.Fatalf("payload items: got %d, want 1", len(is.Commitment.Payload))
	}
	if is.Commitment.Payload[0].PayloadID != [2]byte{'m', 'h'} {
		t.Errorf("payload id mismatch")
	}
}

func TestBuildInitialSubmission_sparseSigners(t *testing.T) {
	// Indices 1 and 3 sign out of 4 — the bitfield should reflect that
	// exactly, and the seed should be the lowest-index signer (1).
	commit, addrs := buildSignedCommitment(t, []int{1, 3})
	tree, err := validators.New(addrs)
	if err != nil {
		t.Fatalf("tree: %v", err)
	}

	is, err := BuildInitialSubmission(commit, tree)
	if err != nil {
		t.Fatalf("build: %v", err)
	}

	if bitfield.Count(is.Bitfield) != 2 {
		t.Errorf("bitfield count: got %d, want 2", bitfield.Count(is.Bitfield))
	}
	for _, want := range []int{1, 3} {
		if !bitfield.IsSet(is.Bitfield, want) {
			t.Errorf("bitfield bit %d: not set", want)
		}
	}
	for _, want := range []int{0, 2} {
		if bitfield.IsSet(is.Bitfield, want) {
			t.Errorf("bitfield bit %d: should not be set", want)
		}
	}

	if is.Proof.Index.Cmp(big.NewInt(1)) != 0 {
		t.Errorf("seed index: got %s, want 1 (lowest present)", is.Proof.Index.String())
	}
	if [20]byte(is.Proof.Account) != addrs[1] {
		t.Errorf("seed account: got %x, want validator 1's %x", is.Proof.Account, addrs[1])
	}
}

func TestBuildInitialSubmission_rejectsZeroSignatures(t *testing.T) {
	commit, addrs := buildSignedCommitment(t, nil)
	tree, _ := validators.New(addrs)
	if _, err := BuildInitialSubmission(commit, tree); err == nil {
		t.Fatal("expected error for zero signatures")
	}
}

func TestBuildInitialSubmission_rejectsTreeSizeMismatch(t *testing.T) {
	commit, _ := buildSignedCommitment(t, []int{0})
	tree, _ := validators.New([][20]byte{{1}, {2}}) // wrong size
	if _, err := BuildInitialSubmission(commit, tree); err == nil {
		t.Fatal("expected error on tree size mismatch")
	}
}

func TestBuildInitialSubmission_seedProofVerifies(t *testing.T) {
	// End-to-end: build the submission, then ecrecover the seed proof's
	// (v, r, s) against the commitment hash → must match the seed account.
	commit, addrs := buildSignedCommitment(t, []int{2})
	tree, _ := validators.New(addrs)

	is, err := BuildInitialSubmission(commit, tree)
	if err != nil {
		t.Fatalf("build: %v", err)
	}

	var sig beefy.Signature
	copy(sig[0:32], is.Proof.R[:])
	copy(sig[32:64], is.Proof.S[:])
	sig[64] = is.Proof.V - 27 // back to raw {0,1} for our helper

	got, err := proof.RecoverAddress(is.CommitmentHash, sig)
	if err != nil {
		t.Fatalf("recover: %v", err)
	}
	if got != [20]byte(is.Proof.Account) {
		t.Errorf("recover: got %x, want %x", got, is.Proof.Account)
	}
}
