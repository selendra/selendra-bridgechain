package driver

import (
	"fmt"
	"math/big"

	"github.com/nathselendra/bridgechain/relayer/internal/beefy"
	"github.com/nathselendra/bridgechain/relayer/internal/bindings"
	"github.com/nathselendra/bridgechain/relayer/internal/bitfield"
	"github.com/nathselendra/bridgechain/relayer/internal/mmr"
	"github.com/nathselendra/bridgechain/relayer/internal/proof"
	"github.com/nathselendra/bridgechain/relayer/internal/validators"
)

// FinalSubmission is the input bundle for `BeefyClient.submitFinal`.
//
// `Proofs` has one entry per sampled validator (the indices the contract's
// `createFinalBitfield` view returns), in ascending index order. The
// contract walks the final bitfield in the same order and matches each
// set bit to the next ValidatorProof; mismatched ordering is rejected
// as `InvalidValidatorProof`.
type FinalSubmission struct {
	Commitment     bindings.BeefyClientCommitment
	Bitfield       []*big.Int
	Proofs         []bindings.BeefyClientValidatorProof
	Leaf           bindings.BeefyClientMMRLeaf
	LeafProof      [][32]byte
	LeafProofOrder *big.Int
	CommitmentHash [32]byte
}

// BuildFinalSubmission packages the inputs to `BeefyClient.submitFinal`.
//
//   - `commit`: the same SignedCommitment used in submitInitial.
//   - `tree`: validator-set Merkle tree (same one used in initial).
//   - `finalIndices`: validator indices the contract sampled — comes from
//     calling `BeefyClient.createFinalBitfield(commitmentHash, initialBitfield)`
//     as a view, then extracting indices via `bitfield.Indices`.
//   - `leaf` + `leafProof` + `leafProofOrder`: the MMR proof for the leaf
//     produced for the BEEFY block. Typically built by `mmr.Simplify` on
//     the output of `mmr_generateProof`.
//
// Every index in `finalIndices` must have a present signature in `commit`,
// otherwise we can't build a ValidatorProof for it. The caller should
// only sample indices already known to be in the initial bitfield.
func BuildFinalSubmission(
	commit *beefy.SignedCommitment,
	tree *validators.Tree,
	finalIndices []int,
	leaf mmr.Leaf,
	leafProof [][32]byte,
	leafProofOrder uint64,
) (*FinalSubmission, error) {
	if commit == nil {
		return nil, fmt.Errorf("driver: nil commitment")
	}
	if tree == nil {
		return nil, fmt.Errorf("driver: nil validator tree")
	}
	if tree.Len() != len(commit.Signatures) {
		return nil, fmt.Errorf("driver: validator tree size %d != signature slot count %d",
			tree.Len(), len(commit.Signatures))
	}
	if len(finalIndices) == 0 {
		return nil, fmt.Errorf("driver: empty final-indices set")
	}

	commitmentHash := proof.CommitmentHash(commit.Commitment)

	// Build the final bitfield + per-index ValidatorProofs in the same
	// ascending-index order. The contract requires ordering match.
	bf, err := bitfield.From(finalIndices, len(commit.Signatures))
	if err != nil {
		return nil, fmt.Errorf("driver: build final bitfield: %w", err)
	}

	proofs := make([]bindings.BeefyClientValidatorProof, 0, len(finalIndices))
	for _, idx := range finalIndices {
		if idx < 0 || idx >= len(commit.Signatures) {
			return nil, fmt.Errorf("driver: final index %d out of range", idx)
		}
		sig := commit.Signatures[idx]
		if sig == nil {
			return nil, fmt.Errorf("driver: validator %d sampled but didn't sign", idx)
		}
		vp, err := proof.BuildValidatorProof(commitmentHash, *sig, idx, tree)
		if err != nil {
			return nil, fmt.Errorf("driver: validator %d proof: %w", idx, err)
		}
		proofs = append(proofs, vp)
	}

	return &FinalSubmission{
		Commitment:     toBindingsCommitment(commit.Commitment),
		Bitfield:       bf,
		Proofs:         proofs,
		Leaf:           toBindingsLeaf(leaf),
		LeafProof:      leafProof,
		LeafProofOrder: new(big.Int).SetUint64(leafProofOrder),
		CommitmentHash: commitmentHash,
	}, nil
}

// toBindingsLeaf converts an mmr.Leaf into the abigen-generated struct.
// Field-name mapping: bridgechain calls it `LeafExtra`, the abigen struct
// inherits Snowfork's `ParachainHeadsRoot` name — same 32 bytes, just
// different semantic on a solochain (here it's the outbound-message
// Merkle root, not a parachain heads root).
func toBindingsLeaf(l mmr.Leaf) bindings.BeefyClientMMRLeaf {
	return bindings.BeefyClientMMRLeaf{
		Version:              l.Version,
		ParentNumber:         l.ParentNumber,
		ParentHash:           l.ParentHash,
		NextAuthoritySetID:   l.NextAuthoritySet.ID,
		NextAuthoritySetLen:  l.NextAuthoritySet.Len,
		NextAuthoritySetRoot: l.NextAuthoritySet.Root,
		ParachainHeadsRoot:   l.LeafExtra,
	}
}
