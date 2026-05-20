// Package driver assembles the call arguments for the BeefyClient
// commit-reveal flow.
//
// The state machine itself (submitInitial → wait → commitPrevRandao →
// submitFinal) will sit on top of these pure-function packagers — each
// step takes plain data, returns plain data, and is unit-testable without
// a live chain.
package driver

import (
	"fmt"
	"math/big"

	"github.com/nathselendra/bridgechain/relayer/internal/beefy"
	"github.com/nathselendra/bridgechain/relayer/internal/bindings"
	"github.com/nathselendra/bridgechain/relayer/internal/bitfield"
	"github.com/nathselendra/bridgechain/relayer/internal/proof"
	"github.com/nathselendra/bridgechain/relayer/internal/validators"
)

// InitialSubmission is the input bundle for `BeefyClient.submitInitial`.
// CommitmentHash is included for callers that want to poll the resulting
// ticket (`tickets[createTicketID(msg.sender, commitmentHash)]`) without
// recomputing it.
type InitialSubmission struct {
	Commitment     bindings.BeefyClientCommitment
	Bitfield       []*big.Int
	Proof          bindings.BeefyClientValidatorProof
	CommitmentHash [32]byte
}

// BuildInitialSubmission packages a decoded SignedCommitment + validator
// tree into the inputs `submitInitial` consumes.
//
//   - Bitfield bit `i` set ⇔ signature slot `i` is non-nil in `commit`.
//     This is the "initial bitfield" the contract stores and from which
//     it later samples (via prevRandao) the subset to reveal.
//
//   - The seed `Proof` is the first present signature. The contract
//     verifies it before recording the ticket — picking *any* present
//     signature is sufficient; the choice doesn't affect later sampling.
//
// The function does not submit anything itself. The driver hands the
// returned data to `BeefyClientTransactor.SubmitInitial`.
func BuildInitialSubmission(
	commit *beefy.SignedCommitment,
	tree *validators.Tree,
) (*InitialSubmission, error) {
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

	// Collect the indices of present signatures + the seed signature.
	var (
		setIndices []int
		seedIdx    = -1
		seedSig    beefy.Signature
	)
	for i, sig := range commit.Signatures {
		if sig == nil {
			continue
		}
		setIndices = append(setIndices, i)
		if seedIdx == -1 {
			seedIdx = i
			seedSig = *sig
		}
	}
	if seedIdx == -1 {
		return nil, fmt.Errorf("driver: commitment has zero signatures")
	}

	bf, err := bitfield.From(setIndices, len(commit.Signatures))
	if err != nil {
		return nil, fmt.Errorf("driver: build bitfield: %w", err)
	}

	commitmentHash := proof.CommitmentHash(commit.Commitment)
	seedProof, err := proof.BuildValidatorProof(commitmentHash, seedSig, seedIdx, tree)
	if err != nil {
		return nil, fmt.Errorf("driver: seed proof: %w", err)
	}

	return &InitialSubmission{
		Commitment:     toBindingsCommitment(commit.Commitment),
		Bitfield:       bf,
		Proof:          seedProof,
		CommitmentHash: commitmentHash,
	}, nil
}

// toBindingsCommitment converts the SCALE-decoded Commitment from the
// `beefy` package into the abigen-generated struct the binding expects.
// The two types carry the same data — different package homes only.
func toBindingsCommitment(c beefy.Commitment) bindings.BeefyClientCommitment {
	payload := make([]bindings.BeefyClientPayloadItem, len(c.Payload))
	for i, item := range c.Payload {
		payload[i] = bindings.BeefyClientPayloadItem{
			PayloadID: item.ID,
			Data:      append([]byte(nil), item.Data...),
		}
	}
	return bindings.BeefyClientCommitment{
		BlockNumber:    c.BlockNumber,
		ValidatorSetID: c.ValidatorSetID,
		Payload:        payload,
	}
}
