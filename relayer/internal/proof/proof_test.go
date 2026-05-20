package proof

import (
	"bytes"
	"encoding/hex"
	"testing"

	ethcrypto "github.com/ethereum/go-ethereum/crypto"
	"github.com/nathselendra/bridgechain/relayer/internal/beefy"
	"github.com/nathselendra/bridgechain/relayer/internal/validators"
	"golang.org/x/crypto/sha3"
)

// Hand-built reference: SCALE-encoding `Commitment` with one MMR-root
// payload (id "mh", 32 bytes of 0xab), block_number=42, validator_set_id=7.
//
// Layout (in order): payload, block_number, validator_set_id.
//   payload: compact(1)=0x04, id="mh", compact(32)=0x80, 32×0xab
//   block_number: 42 (u32 LE)
//   validator_set_id: 7 (u64 LE)
func referenceCommitment() (beefy.Commitment, []byte) {
	data := bytes.Repeat([]byte{0xab}, 32)
	c := beefy.Commitment{
		Payload:        []beefy.PayloadItem{{ID: [2]byte{'m', 'h'}, Data: data}},
		BlockNumber:    42,
		ValidatorSetID: 7,
	}
	expected := []byte{0x04, 'm', 'h', 0x80}
	expected = append(expected, data...)
	expected = append(expected, 0x2a, 0x00, 0x00, 0x00)                   // u32 LE = 42
	expected = append(expected, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00) // u64 LE = 7
	return c, expected
}

func TestEncodeCommitment_matchesReference(t *testing.T) {
	c, want := referenceCommitment()
	got := EncodeCommitment(c)
	if !bytes.Equal(got, want) {
		t.Errorf("encoding mismatch\nwant: %s\ngot:  %s",
			hex.EncodeToString(want), hex.EncodeToString(got))
	}
}

func TestCommitmentHash_isKeccakOfScale(t *testing.T) {
	c, _ := referenceCommitment()
	got := CommitmentHash(c)

	h := sha3.NewLegacyKeccak256()
	h.Write(EncodeCommitment(c))
	var want [32]byte
	copy(want[:], h.Sum(nil))

	if got != want {
		t.Errorf("hash mismatch\nwant: %x\ngot:  %x", want, got)
	}
}

// signOnce wraps a known private key signing `hash` and returns a
// substrate-style 65-byte signature: r ‖ s ‖ v where v ∈ {0,1}.
func signOnce(t *testing.T, hash [32]byte, hexKey string) (beefy.Signature, [20]byte) {
	t.Helper()
	key, err := ethcrypto.HexToECDSA(hexKey)
	if err != nil {
		t.Fatalf("hex key: %v", err)
	}
	sigBytes, err := ethcrypto.Sign(hash[:], key)
	if err != nil {
		t.Fatalf("sign: %v", err)
	}
	if len(sigBytes) != 65 {
		t.Fatalf("sig length: %d", len(sigBytes))
	}
	var sig beefy.Signature
	copy(sig[:], sigBytes) // go-ethereum already returns r ‖ s ‖ {0,1}
	var addr [20]byte
	copy(addr[:], ethcrypto.PubkeyToAddress(key.PublicKey).Bytes())
	return sig, addr
}

func TestRecoverAddress_matchesSigner(t *testing.T) {
	// Deterministic test key — Anvil's index-0 private key.
	const hexKey = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
	c, _ := referenceCommitment()
	hash := CommitmentHash(c)
	sig, want := signOnce(t, hash, hexKey)

	got, err := RecoverAddress(hash, sig)
	if err != nil {
		t.Fatalf("recover: %v", err)
	}
	if got != want {
		t.Errorf("address: got %x, want %x", got, want)
	}
}

func TestRecoverAddress_rejectsBadV(t *testing.T) {
	c, _ := referenceCommitment()
	hash := CommitmentHash(c)
	var sig beefy.Signature
	sig[64] = 27 // already in Ethereum form — RecoverAddress wants raw {0,1}
	if _, err := RecoverAddress(hash, sig); err == nil {
		t.Fatal("expected error on out-of-range v")
	}
}

// verifyMerkleRoot mirrors SubstrateMerkleProof.computeRoot from
// contracts/src/utils/SubstrateMerkleProof.sol. Identical to the helper
// in internal/validators/tree_test.go — kept local to avoid widening that
// package's surface.
func verifyMerkleRoot(leaf [32]byte, position, width int, sibs [][32]byte) [32]byte {
	node := leaf
	for _, sib := range sibs {
		if position&1 == 1 || position+1 == width {
			node = keccakPair(sib, node)
		} else {
			node = keccakPair(node, sib)
		}
		position >>= 1
		width = ((width - 1) >> 1) + 1
	}
	return node
}

func keccakPair(a, b [32]byte) [32]byte {
	h := sha3.NewLegacyKeccak256()
	h.Write(a[:])
	h.Write(b[:])
	var out [32]byte
	copy(out[:], h.Sum(nil))
	return out
}

func keccakOne(b [20]byte) [32]byte {
	h := sha3.NewLegacyKeccak256()
	h.Write(b[:])
	var out [32]byte
	copy(out[:], h.Sum(nil))
	return out
}

func TestBuildValidatorProof_singleValidator(t *testing.T) {
	const hexKey = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
	c, _ := referenceCommitment()
	hash := CommitmentHash(c)
	sig, signer := signOnce(t, hash, hexKey)

	tree, err := validators.New([][20]byte{signer})
	if err != nil {
		t.Fatalf("validator tree: %v", err)
	}

	vp, err := BuildValidatorProof(hash, sig, 0, tree)
	if err != nil {
		t.Fatalf("build: %v", err)
	}

	// v must be in Ethereum form, not raw {0,1}.
	if vp.V != 27 && vp.V != 28 {
		t.Errorf("v: got %d, want 27 or 28", vp.V)
	}
	// recovered account == signer
	if [20]byte(vp.Account) != signer {
		t.Errorf("account: got %x, want %x", vp.Account.Bytes(), signer)
	}
	if vp.Index.Int64() != 0 {
		t.Errorf("index: got %d, want 0", vp.Index.Int64())
	}
	// Single-validator tree → empty proof + root == leaf
	if len(vp.Proof) != 0 {
		t.Errorf("proof: want empty, got %d items", len(vp.Proof))
	}
	root := verifyMerkleRoot(keccakOne(signer), 0, 1, vp.Proof)
	if root != tree.Root() {
		t.Errorf("merkle verify: got %x, want %x", root, tree.Root())
	}
}

func TestBuildValidatorProofs_skipsAbsentSlots(t *testing.T) {
	// Three validators, only #0 and #2 sign. Verify the output skips #1.
	keys := []string{
		"ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
		"59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
		"5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
	}
	c, _ := referenceCommitment()
	hash := CommitmentHash(c)

	var addrs [][20]byte
	var sigs [3]*beefy.Signature
	for i, k := range keys {
		sig, addr := signOnce(t, hash, k)
		addrs = append(addrs, addr)
		if i == 1 {
			continue // pretend validator 1 didn't sign
		}
		s := sig
		sigs[i] = &s
	}

	commit := &beefy.SignedCommitment{Commitment: c, Signatures: sigs[:]}
	tree, err := validators.New(addrs)
	if err != nil {
		t.Fatalf("tree: %v", err)
	}

	proofs, err := BuildValidatorProofs(commit, tree)
	if err != nil {
		t.Fatalf("build: %v", err)
	}
	if len(proofs) != 2 {
		t.Fatalf("expected 2 proofs (skip validator 1), got %d", len(proofs))
	}
	gotIdx := []int64{proofs[0].Index.Int64(), proofs[1].Index.Int64()}
	if gotIdx[0] != 0 || gotIdx[1] != 2 {
		t.Errorf("indices: got %v, want [0 2]", gotIdx)
	}

	// Each proof must Merkle-verify against the same root.
	width := tree.Len()
	for _, vp := range proofs {
		leaf := keccakOne([20]byte(vp.Account))
		root := verifyMerkleRoot(leaf, int(vp.Index.Int64()), width, vp.Proof)
		if root != tree.Root() {
			t.Errorf("validator %d: merkle verify failed", vp.Index.Int64())
		}
		// Each proof must also recover to its claimed account.
		var sig beefy.Signature
		copy(sig[0:32], vp.R[:])
		copy(sig[32:64], vp.S[:])
		sig[64] = vp.V - 27 // back to raw {0,1} for our helper
		got, err := RecoverAddress(hash, sig)
		if err != nil {
			t.Errorf("recover: %v", err)
			continue
		}
		if got != [20]byte(vp.Account) {
			t.Errorf("validator %d: recover mismatch", vp.Index.Int64())
		}
	}
}

func TestBuildValidatorProofs_rejectsSizeMismatch(t *testing.T) {
	c, _ := referenceCommitment()
	commit := &beefy.SignedCommitment{
		Commitment: c,
		Signatures: make([]*beefy.Signature, 2),
	}
	tree, _ := validators.New([][20]byte{{0x01}, {0x02}, {0x03}})

	if _, err := BuildValidatorProofs(commit, tree); err == nil {
		t.Fatal("expected size mismatch error")
	}
}
