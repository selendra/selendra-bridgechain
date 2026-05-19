// Bridgechain relayer entry point.
//
// Two long-running loops:
//
//   - Substrate → Ethereum: subscribe to BEEFY justifications on the
//     bridgechain node, decode the SignedCommitment, and submit it (with an
//     MMR leaf proof for the latest finalized block) to the BeefyClient
//     contract on Ethereum.
//
//   - Ethereum → Substrate: watch the Gateway contract for
//     OutboundMessageAccepted events, build the receipt MPT proof, and
//     submit to the bridge-inbound pallet.
//
// This file is the skeleton: both loops run, but submission is stubbed (logs
// what would be submitted). The signature aggregation, commit-reveal flow,
// and beacon-chain proofs are TODO — each gets its own follow-up.
package main

import (
	"context"
	"flag"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"syscall"

	"github.com/nathselendra/bridgechain/relayer/internal/beefy"
	"github.com/nathselendra/bridgechain/relayer/internal/ethereum"
	"github.com/nathselendra/bridgechain/relayer/internal/substrate"
)

type config struct {
	substrateRPC   string
	ethereumRPC    string
	gatewayAddress string
	beefyAddress   string
}

func parseFlags() config {
	var c config
	flag.StringVar(&c.substrateRPC, "substrate-rpc", envOr("BRIDGECHAIN_SUBSTRATE_RPC", "ws://127.0.0.1:9944"),
		"WebSocket RPC of the bridgechain node")
	flag.StringVar(&c.ethereumRPC, "ethereum-rpc", envOr("BRIDGECHAIN_ETHEREUM_RPC", "ws://127.0.0.1:8545"),
		"WebSocket RPC of the Ethereum execution-layer node (Anvil or production)")
	flag.StringVar(&c.gatewayAddress, "gateway", envOr("BRIDGECHAIN_GATEWAY_ADDRESS", ""),
		"Deployed Gateway.sol address (0x... — empty in skeleton mode)")
	flag.StringVar(&c.beefyAddress, "beefy-client", envOr("BRIDGECHAIN_BEEFY_CLIENT_ADDRESS", ""),
		"Deployed BeefyClient.sol address (0x... — empty in skeleton mode)")
	flag.Parse()
	return c
}

func envOr(key, fallback string) string {
	if v, ok := os.LookupEnv(key); ok {
		return v
	}
	return fallback
}

func main() {
	cfg := parseFlags()

	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelDebug}))
	slog.SetDefault(logger)

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	subClient, err := substrate.Dial(ctx, cfg.substrateRPC)
	if err != nil {
		fail("dial substrate", err)
	}
	defer subClient.Close()

	ethClient, err := ethereum.Dial(ctx, cfg.ethereumRPC)
	if err != nil {
		fail("dial ethereum", err)
	}
	defer ethClient.Close()

	slog.Info("relayer up",
		"substrate", cfg.substrateRPC,
		"ethereum", cfg.ethereumRPC,
		"gateway", strOr(cfg.gatewayAddress, "<unset>"),
		"beefy_client", strOr(cfg.beefyAddress, "<unset>"))

	relay := beefy.NewRelay(subClient, ethClient, cfg.beefyAddress)
	if err := relay.Run(ctx); err != nil {
		fail("beefy relay", err)
	}
}

func strOr(s, fallback string) string {
	if s == "" {
		return fallback
	}
	return s
}

func fail(stage string, err error) {
	fmt.Fprintf(os.Stderr, "relayer: %s: %v\n", stage, err)
	os.Exit(1)
}
