package driver

import (
	"context"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/ethclient"
)

// EthClientChain adapts an `*ethclient.Client` to the driver's EthChain
// interface. Two trivial impedance-matching bits:
//
//   - TransactionReceipt takes common.Hash on the ethclient side; the
//     driver uses [32]byte to keep its interface dep-free.
//   - go-ethereum returns `ethereum.NotFound` for pending txs; the
//     driver tolerates non-nil errors from this method and keeps polling.
type EthClientChain struct {
	C *ethclient.Client
}

func (e *EthClientChain) BlockNumber(ctx context.Context) (uint64, error) {
	return e.C.BlockNumber(ctx)
}

func (e *EthClientChain) TransactionReceipt(
	ctx context.Context, txHash [32]byte,
) (*types.Receipt, error) {
	return e.C.TransactionReceipt(ctx, common.Hash(txHash))
}
