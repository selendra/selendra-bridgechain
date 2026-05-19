// Package ethereum is a thin wrapper around go-ethereum's ethclient.
//
// Future iterations will hold the BeefyClient and Gateway contract bindings
// generated via `abigen`. For the skeleton we only need a working WebSocket
// connection and the ability to read chain ID / block number.
package ethereum

import (
	"context"
	"fmt"
	"math/big"

	"github.com/ethereum/go-ethereum/ethclient"
)

// Client wraps an ethclient.Client.
type Client struct {
	*ethclient.Client
	chainID *big.Int
}

// Dial opens a WebSocket connection to an Ethereum node.
func Dial(ctx context.Context, url string) (*Client, error) {
	raw, err := ethclient.DialContext(ctx, url)
	if err != nil {
		return nil, fmt.Errorf("ethereum: dial %s: %w", url, err)
	}
	chainID, err := raw.ChainID(ctx)
	if err != nil {
		raw.Close()
		return nil, fmt.Errorf("ethereum: chain id: %w", err)
	}
	return &Client{Client: raw, chainID: chainID}, nil
}

// ChainID returns the chain ID discovered at Dial time.
func (c *Client) ChainID() *big.Int {
	return new(big.Int).Set(c.chainID)
}
