package ethereum

import (
	"context"
	"crypto/ecdsa"
	"fmt"
	"strings"

	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/crypto"
)

// Transactor signs and submits transactions on behalf of the relayer
// operator. Wraps `bind.TransactOpts` so callers see a single struct.
//
// The key is held in memory for the lifetime of the relayer process —
// production deployments will likely want to swap this for a keystore or a
// hardware-wallet backed `bind.SignerFn`. Keep the surface area small so
// that swap is local.
type Transactor struct {
	Address common.Address
	Opts    *bind.TransactOpts
}

// NewTransactor loads a hex-encoded private key (with or without `0x`
// prefix) and prepares it for sending transactions to `chainID`.
//
// Don't log the key. Callers should pass values from env vars or a
// keystore — never from a config file checked into git.
func NewTransactor(privKeyHex string, chainID *Client) (*Transactor, error) {
	keyStr := strings.TrimPrefix(privKeyHex, "0x")
	key, err := crypto.HexToECDSA(keyStr)
	if err != nil {
		return nil, fmt.Errorf("ethereum: parse private key: %w", err)
	}
	pub, ok := key.Public().(*ecdsa.PublicKey)
	if !ok {
		return nil, fmt.Errorf("ethereum: unexpected public key type")
	}
	addr := crypto.PubkeyToAddress(*pub)

	opts, err := bind.NewKeyedTransactorWithChainID(key, chainID.ChainID())
	if err != nil {
		return nil, fmt.Errorf("ethereum: build transactor: %w", err)
	}
	// Default to estimating gas at call time. Override per-tx as needed.
	opts.GasLimit = 0
	return &Transactor{Address: addr, Opts: opts}, nil
}

// WithContext returns a copy of the underlying TransactOpts that carries
// `ctx` so cancellation propagates into go-ethereum's RPC calls.
func (t *Transactor) WithContext(ctx context.Context) *bind.TransactOpts {
	cp := *t.Opts
	cp.Context = ctx
	return &cp
}
