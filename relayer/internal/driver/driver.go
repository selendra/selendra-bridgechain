package driver

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"math/big"
	"time"

	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/nathselendra/bridgechain/relayer/internal/beefy"
	"github.com/nathselendra/bridgechain/relayer/internal/bindings"
	"github.com/nathselendra/bridgechain/relayer/internal/bitfield"
	"github.com/nathselendra/bridgechain/relayer/internal/mmr"
	"github.com/nathselendra/bridgechain/relayer/internal/validators"
)

// EthBackend is the subset of the abigen-generated BeefyClient API the
// driver needs. Defined as an interface so unit tests can swap in a fake.
type EthBackend interface {
	SubmitInitial(
		opts *bind.TransactOpts,
		commitment bindings.BeefyClientCommitment,
		bitfield []*big.Int,
		proof bindings.BeefyClientValidatorProof,
	) (*types.Transaction, error)

	CommitPrevRandao(
		opts *bind.TransactOpts,
		commitmentHash [32]byte,
	) (*types.Transaction, error)

	CreateFinalBitfield(
		opts *bind.CallOpts,
		commitmentHash [32]byte,
		bitfield []*big.Int,
	) ([]*big.Int, error)

	SubmitFinal(
		opts *bind.TransactOpts,
		commitment bindings.BeefyClientCommitment,
		bitfield []*big.Int,
		proofs []bindings.BeefyClientValidatorProof,
		leaf bindings.BeefyClientMMRLeaf,
		leafProof [][32]byte,
		leafProofOrder *big.Int,
	) (*types.Transaction, error)
}

// EthChain is the chain-state surface the driver needs — block height
// and receipt polling. Split from EthBackend because the abigen-generated
// BeefyClient struct doesn't expose either.
type EthChain interface {
	BlockNumber(ctx context.Context) (uint64, error)
	TransactionReceipt(ctx context.Context, txHash [32]byte) (*types.Receipt, error)
}

// Driver runs one BEEFY commit-reveal cycle. It is intentionally
// stateless — each Relay invocation handles exactly one commitment from
// start to finish. A higher-level loop picks commitments and decides
// which to relay; this layer just executes the protocol once chosen.
type Driver struct {
	Beefy        EthBackend
	Chain        EthChain
	TxOpts       *bind.TransactOpts
	RandaoDelay  uint64        // blocks; matches BeefyClient.randaoCommitDelay
	PollInterval time.Duration // how often to re-check the ETH head while waiting
	Log          *slog.Logger
}

// Relay performs all four protocol phases against `Beefy`:
//
//  1. submitInitial(commitment, initial bitfield, seed validator proof)
//  2. wait for `RandaoDelay` ETH blocks
//  3. commitPrevRandao(commitmentHash)
//  4. createFinalBitfield (view) → submitFinal(...)
//
// The MMR leaf + proof + proofOrder must be supplied by the caller —
// typically the result of `mmr_generateProof` for the BEEFY block
// followed by `mmr.Simplify` to flatten the multi-peak proof.
//
// `Relay` returns nil on success. Any error is returned immediately;
// the caller decides whether to retry (the contract's ticket-based
// flow lets a new `submitInitial` start a fresh attempt).
func (d *Driver) Relay(
	ctx context.Context,
	commit *beefy.SignedCommitment,
	tree *validators.Tree,
	leaf mmr.Leaf,
	leafProof [][32]byte,
	leafProofOrder uint64,
) error {
	log := d.logger()

	initial, err := BuildInitialSubmission(commit, tree)
	if err != nil {
		return fmt.Errorf("driver: initial submission: %w", err)
	}

	startBlock, err := d.submitInitialAndWait(ctx, initial)
	if err != nil {
		return err
	}
	log.Info("driver: submitInitial mined",
		"commitment_hash", fmt.Sprintf("%x", initial.CommitmentHash),
		"block", startBlock)

	if err := d.waitBlocks(ctx, startBlock+d.RandaoDelay); err != nil {
		return fmt.Errorf("driver: wait randao delay: %w", err)
	}

	if err := d.commitPrevRandaoAndWait(ctx, initial.CommitmentHash); err != nil {
		return err
	}
	log.Info("driver: commitPrevRandao mined")

	finalIndices, err := d.sampleFinalIndices(ctx, initial)
	if err != nil {
		return fmt.Errorf("driver: sample final indices: %w", err)
	}
	log.Info("driver: sampled validators", "indices", finalIndices)

	final, err := BuildFinalSubmission(commit, tree, finalIndices,
		leaf, leafProof, leafProofOrder)
	if err != nil {
		return fmt.Errorf("driver: final submission: %w", err)
	}

	if err := d.submitFinalAndWait(ctx, final); err != nil {
		return err
	}
	log.Info("driver: submitFinal mined — bridge advanced")
	return nil
}

// submitInitialAndWait sends submitInitial, waits for the receipt, and
// returns the block number it was mined in. Errors on any non-success
// receipt — the contract reverts are surfaced verbatim.
func (d *Driver) submitInitialAndWait(
	ctx context.Context, initial *InitialSubmission,
) (uint64, error) {
	tx, err := d.Beefy.SubmitInitial(d.txOpts(ctx),
		initial.Commitment, initial.Bitfield, initial.Proof)
	if err != nil {
		return 0, fmt.Errorf("driver: submitInitial: %w", err)
	}
	r, err := d.waitReceipt(ctx, tx)
	if err != nil {
		return 0, err
	}
	return r.BlockNumber.Uint64(), nil
}

func (d *Driver) commitPrevRandaoAndWait(
	ctx context.Context, commitmentHash [32]byte,
) error {
	tx, err := d.Beefy.CommitPrevRandao(d.txOpts(ctx), commitmentHash)
	if err != nil {
		return fmt.Errorf("driver: commitPrevRandao: %w", err)
	}
	if _, err := d.waitReceipt(ctx, tx); err != nil {
		return err
	}
	return nil
}

func (d *Driver) sampleFinalIndices(
	ctx context.Context, initial *InitialSubmission,
) ([]int, error) {
	// BeefyClient keys tickets by `msg.sender`. For view calls, abigen
	// defaults CallOpts.From to address(0), which would look up a
	// different (non-existent) ticket — set it to the operator address
	// so the lookup hits the ticket we just created in submitInitial.
	out, err := d.Beefy.CreateFinalBitfield(
		&bind.CallOpts{Context: ctx, From: d.TxOpts.From},
		initial.CommitmentHash,
		initial.Bitfield,
	)
	if err != nil {
		return nil, fmt.Errorf("driver: createFinalBitfield: %w", err)
	}
	return bitfield.Indices(out), nil
}

func (d *Driver) submitFinalAndWait(
	ctx context.Context, final *FinalSubmission,
) error {
	tx, err := d.Beefy.SubmitFinal(d.txOpts(ctx),
		final.Commitment, final.Bitfield, final.Proofs,
		final.Leaf, final.LeafProof, final.LeafProofOrder)
	if err != nil {
		return fmt.Errorf("driver: submitFinal: %w", err)
	}
	if _, err := d.waitReceipt(ctx, tx); err != nil {
		return err
	}
	return nil
}

// waitBlocks polls Chain.BlockNumber until it reaches `target`.
func (d *Driver) waitBlocks(ctx context.Context, target uint64) error {
	interval := d.PollInterval
	if interval <= 0 {
		interval = time.Second
	}
	for {
		current, err := d.Chain.BlockNumber(ctx)
		if err != nil {
			return fmt.Errorf("driver: block number: %w", err)
		}
		if current >= target {
			return nil
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(interval):
		}
	}
}

// waitReceipt polls TransactionReceipt until the tx is mined.
// `nil` receipt returns from go-ethereum mean "not yet mined".
func (d *Driver) waitReceipt(
	ctx context.Context, tx *types.Transaction,
) (*types.Receipt, error) {
	interval := d.PollInterval
	if interval <= 0 {
		interval = time.Second
	}
	hash := tx.Hash()
	var h32 [32]byte
	copy(h32[:], hash.Bytes())
	for {
		r, err := d.Chain.TransactionReceipt(ctx, h32)
		if err == nil && r != nil {
			if r.Status == types.ReceiptStatusSuccessful {
				return r, nil
			}
			return nil, fmt.Errorf("driver: tx %x reverted (status %d, gas %d)",
				h32, r.Status, r.GasUsed)
		}
		// Some backends return a typed "not found" error; treat it as
		// "keep polling" rather than terminal.
		if err != nil && !errors.Is(err, ErrTxNotMined) {
			// Many go-ethereum backends return ethereum.NotFound as a
			// plain error wrapping nothing recognizable. Be permissive:
			// keep polling until ctx expires.
			_ = err
		}
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(interval):
		}
	}
}

// ErrTxNotMined is a sentinel some EthChain implementations may return
// from TransactionReceipt while a tx is pending. Production go-ethereum
// uses ethereum.NotFound; we tolerate either.
var ErrTxNotMined = errors.New("driver: tx not yet mined")

func (d *Driver) txOpts(ctx context.Context) *bind.TransactOpts {
	// bind.TransactOpts is not goroutine-safe; the driver runs one
	// commit-reveal cycle at a time so a single shared opts is fine.
	// We override the Context per call so cancellation propagates.
	opts := *d.TxOpts
	opts.Context = ctx
	return &opts
}

func (d *Driver) logger() *slog.Logger {
	if d.Log != nil {
		return d.Log
	}
	return slog.Default()
}

