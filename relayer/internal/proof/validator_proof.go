package proof

import (
	"fmt"
	"math/big"

	"github.com/ethereum/go-ethereum/common"
	ethcrypto "github.com/ethereum/go-ethereum/crypto"
	"github.com/nathselendra/bridgechain/relayer/internal/beefy"
	"github.com/nathselendra/bridgechain/relayer/internal/bindings"
	"github.com/nathselendra/bridgechain/relayer/internal/validators"
)

// RecoverAddress recovers the 20-byte Ethereum address that signed
// `commitmentHash` with `sig`. BEEFY signatures are laid out (r ‖ s ‖ v)
// with v ∈ {0, 1}, the same recovery-id convention go-ethereum's crypto
// package uses internally — `Ecrecover` consumes that raw form, not the
// EIP-155 {27,28} v.
func RecoverAddress(commitmentHash [32]byte, sig beefy.Signature) ([20]byte, error) {
	if sig[64] > 1 {
		return [20]byte{}, fmt.Errorf("proof: invalid recovery id %d (want 0 or 1)", sig[64])
	}
	pub, err := ethcrypto.Ecrecover(commitmentHash[:], sig[:])
	if err != nil {
		return [20]byte{}, fmt.Errorf("proof: ecrecover: %w", err)
	}
	// Ecrecover returns 65 bytes: 0x04 prefix + 64 bytes uncompressed pubkey.
	// PubkeyToAddress needs the parsed key.
	parsed, err := ethcrypto.UnmarshalPubkey(pub)
	if err != nil {
		return [20]byte{}, fmt.Errorf("proof: unmarshal pubkey: %w", err)
	}
	var out [20]byte
	copy(out[:], ethcrypto.PubkeyToAddress(*parsed).Bytes())
	return out, nil
}

// BuildValidatorProof packages the data BeefyClient needs to verify that
// validator `index` signed `commitmentHash`: the (v,r,s) split with v
// shifted into the Ethereum convention, the recovered address, and the
// Merkle inclusion proof into the validator-set tree.
//
// `tree` must have been built from the same address list (and in the same
// order) BEEFY emitted — see [`validators.New`].
func BuildValidatorProof(
	commitmentHash [32]byte,
	sig beefy.Signature,
	index int,
	tree *validators.Tree,
) (bindings.BeefyClientValidatorProof, error) {
	addr, err := RecoverAddress(commitmentHash, sig)
	if err != nil {
		return bindings.BeefyClientValidatorProof{}, err
	}
	merkleProof, err := tree.Proof(index)
	if err != nil {
		return bindings.BeefyClientValidatorProof{}, fmt.Errorf("proof: merkle: %w", err)
	}

	var r, s [32]byte
	copy(r[:], sig[0:32])
	copy(s[:], sig[32:64])
	// Substrate uses raw {0,1} recovery ids; Solidity's ECDSA.recover (and
	// EVM ecrecover) take v ∈ {27,28}. Shift here so the proof can be sent
	// straight to the contract.
	v := sig[64] + 27

	return bindings.BeefyClientValidatorProof{
		V:       v,
		R:       r,
		S:       s,
		Index:   big.NewInt(int64(index)),
		Account: common.Address(addr),
		Proof:   merkleProof,
	}, nil
}

// BuildValidatorProofs walks every signature slot in `commit` and produces
// a ValidatorProof for each *present* signature. Missing signatures are
// skipped; the returned slice's indices correspond to validator indices
// (i.e., the contract's `proof.index` field), not to position in the
// returned slice.
//
// The output is ordered by ascending validator index, which is the order
// the contract's bitfield-walking logic expects.
func BuildValidatorProofs(
	commit *beefy.SignedCommitment,
	tree *validators.Tree,
) ([]bindings.BeefyClientValidatorProof, error) {
	if commit == nil {
		return nil, fmt.Errorf("proof: nil commitment")
	}
	if tree == nil {
		return nil, fmt.Errorf("proof: nil validator tree")
	}
	if tree.Len() != len(commit.Signatures) {
		return nil, fmt.Errorf("proof: validator tree size %d != signature slot count %d",
			tree.Len(), len(commit.Signatures))
	}

	commitmentHash := CommitmentHash(commit.Commitment)

	out := make([]bindings.BeefyClientValidatorProof, 0, commit.SignatureCount())
	for i, sig := range commit.Signatures {
		if sig == nil {
			continue
		}
		vp, err := BuildValidatorProof(commitmentHash, *sig, i, tree)
		if err != nil {
			return nil, fmt.Errorf("proof: validator %d: %w", i, err)
		}
		out = append(out, vp)
	}
	return out, nil
}
