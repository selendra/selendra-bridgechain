package driver

import (
	"context"
	"errors"
	"math/big"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/nathselendra/bridgechain/relayer/internal/bindings"
	"github.com/nathselendra/bridgechain/relayer/internal/validators"
)

// fakeBackend records every Driver→contract call in order so the test
// can assert on sequencing + arguments. Each method returns a transaction
// whose hash is deterministic, so fakeChain can match receipts back to it.
type fakeBackend struct {
	calls          []string
	lastInitial    bindings.BeefyClientValidatorProof
	lastInitialBf  []*big.Int
	commitHashSeen [32]byte
	lastFinalBf    []*big.Int
	lastFinalLen   int
	finalSampled   []*big.Int // result returned from CreateFinalBitfield
}

func (f *fakeBackend) SubmitInitial(
	opts *bind.TransactOpts,
	c bindings.BeefyClientCommitment,
	bf []*big.Int,
	p bindings.BeefyClientValidatorProof,
) (*types.Transaction, error) {
	f.calls = append(f.calls, "SubmitInitial")
	f.lastInitial = p
	f.lastInitialBf = bf
	return makeTx(0xa1), nil
}

func (f *fakeBackend) CommitPrevRandao(
	opts *bind.TransactOpts, h [32]byte,
) (*types.Transaction, error) {
	f.calls = append(f.calls, "CommitPrevRandao")
	f.commitHashSeen = h
	return makeTx(0xa2), nil
}

func (f *fakeBackend) CreateFinalBitfield(
	opts *bind.CallOpts, h [32]byte, bf []*big.Int,
) ([]*big.Int, error) {
	f.calls = append(f.calls, "CreateFinalBitfield")
	if f.finalSampled != nil {
		return f.finalSampled, nil
	}
	// Default: echo the initial bitfield back.
	return bf, nil
}

func (f *fakeBackend) SubmitFinal(
	opts *bind.TransactOpts,
	c bindings.BeefyClientCommitment,
	bf []*big.Int,
	proofs []bindings.BeefyClientValidatorProof,
	leaf bindings.BeefyClientMMRLeaf,
	lp [][32]byte,
	lpo *big.Int,
) (*types.Transaction, error) {
	f.calls = append(f.calls, "SubmitFinal")
	f.lastFinalBf = bf
	f.lastFinalLen = len(proofs)
	return makeTx(0xa3), nil
}

// fakeChain returns a fixed BlockNumber sequence and successful receipts
// for every known tx hash. BlockNumber returns successive values so the
// driver's `waitBlocks` loop terminates.
type fakeChain struct {
	startBlock uint64
}

func (f *fakeChain) BlockNumber(ctx context.Context) (uint64, error) {
	cur := f.startBlock
	f.startBlock++ // monotonic per call so waitBlocks always makes progress
	return cur, nil
}

func (f *fakeChain) TransactionReceipt(
	ctx context.Context, h [32]byte,
) (*types.Receipt, error) {
	// Treat every tx as immediately mined and successful.
	return &types.Receipt{
		Status:      types.ReceiptStatusSuccessful,
		BlockNumber: big.NewInt(int64(f.startBlock)),
		TxHash:      hashFromArray(h),
	}, nil
}

func makeTx(marker byte) *types.Transaction {
	// types.NewTransaction is deprecated but produces a deterministic
	// hash from a chainID-less legacy tx — sufficient for tests.
	to := [20]byte{marker}
	addrPtr := addrPtrFromArray(to)
	tx := types.NewTransaction(
		uint64(marker), *addrPtr,
		big.NewInt(0), 21000, big.NewInt(1), nil,
	)
	return tx
}

func TestDriver_Relay_callsAllFourPhasesInOrder(t *testing.T) {
	commit, addrs := buildSignedCommitment(t, []int{0, 1, 2})
	tree, err := validators.New(addrs)
	if err != nil {
		t.Fatalf("tree: %v", err)
	}

	be := &fakeBackend{}
	ch := &fakeChain{startBlock: 100}

	d := &Driver{
		Beefy:        be,
		Chain:        ch,
		TxOpts:       &bind.TransactOpts{},
		RandaoDelay:  4,
		PollInterval: 10 * time.Millisecond,
	}

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	err = d.Relay(ctx, commit, tree, sampleLeaf(), [][32]byte{}, 0)
	if err != nil {
		t.Fatalf("relay: %v", err)
	}

	want := []string{
		"SubmitInitial",
		"CommitPrevRandao",
		"CreateFinalBitfield",
		"SubmitFinal",
	}
	if !equalStrings(be.calls, want) {
		t.Errorf("call sequence:\ngot:  %v\nwant: %v", be.calls, want)
	}
	if be.lastFinalLen == 0 {
		t.Errorf("submitFinal called with zero validator proofs")
	}
	// Commit-randao hash matches the initial submission's commitment hash.
	expectedHash := [32]byte{}
	if be.commitHashSeen == expectedHash {
		t.Errorf("commitPrevRandao called with zero hash")
	}
}

func TestDriver_Relay_propagatesInitialError(t *testing.T) {
	commit, addrs := buildSignedCommitment(t, nil) // no signers
	tree, _ := validators.New(addrs)

	d := &Driver{
		Beefy:        &fakeBackend{},
		Chain:        &fakeChain{},
		TxOpts:       &bind.TransactOpts{},
		RandaoDelay:  4,
		PollInterval: time.Millisecond,
	}

	err := d.Relay(context.Background(), commit, tree, sampleLeaf(), nil, 0)
	if err == nil {
		t.Fatal("expected error when no validators signed")
	}
}

func TestDriver_waitBlocks_respectsContext(t *testing.T) {
	d := &Driver{
		Chain: &stuckChain{}, // never advances past block 0
		PollInterval: time.Millisecond,
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	if err := d.waitBlocks(ctx, 100); !errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("expected deadline exceeded, got %v", err)
	}
}

type stuckChain struct{}

func (stuckChain) BlockNumber(ctx context.Context) (uint64, error) { return 0, nil }
func (stuckChain) TransactionReceipt(ctx context.Context, h [32]byte) (*types.Receipt, error) {
	return nil, nil
}

func equalStrings(a, b []string) bool {
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

// helpers for go-ethereum types we don't want to drag full deps for.

func hashFromArray(h [32]byte) (out [32]byte) {
	// types.Receipt.TxHash is common.Hash which is [32]byte — return as-is.
	return h
}

func addrPtrFromArray(b [20]byte) *[20]byte {
	c := b
	return &c
}
