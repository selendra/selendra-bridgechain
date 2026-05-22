//go:build e2e

// End-to-end test: spins up Anvil + a bridgechain dev node, deploys
// BeefyClient against Anvil with bridgechain's actual validator-set
// root, then drives the full BEEFY commit-reveal cycle. Asserts that
// `BeefyClient.latestMMRRoot()` matches the MMR root inside the
// SignedCommitment the relayer relayed.
//
// Run with:
//   bash relayer/scripts/e2e.sh
// or manually:
//   go test -tags e2e ./internal/driver/ -v -timeout 300s
// after starting anvil on :8545 and bridgechain on :9944 yourself.

package driver

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/big"
	"os"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/crypto"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/nathselendra/bridgechain/relayer/internal/beefy"
	"github.com/nathselendra/bridgechain/relayer/internal/bindings"
	"github.com/nathselendra/bridgechain/relayer/internal/mmr"
	"github.com/nathselendra/bridgechain/relayer/internal/substrate"
	"github.com/nathselendra/bridgechain/relayer/internal/validators"
)

const (
	defaultAnvilURL    = "ws://127.0.0.1:8545"
	defaultBridgeURL   = "ws://127.0.0.1:9944"
	defaultOperatorKey = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
)

func env(name, def string) string {
	if v := os.Getenv(name); v != "" {
		return v
	}
	return def
}

// e2eRig holds the long-lived state needed to relay any number of
// commitments in sequence — the chain clients, the validator tree, the
// deployed BeefyClient binding, and the driver.
type e2eRig struct {
	ctx        context.Context
	t          *testing.T
	eth        *ethclient.Client
	sub        *substrate.Client
	tree       *validators.Tree
	client     *bindings.BeefyClient
	clientAddr common.Address
	driver     *Driver
	stream     <-chan json.RawMessage
}

func setupE2E(t *testing.T, ctx context.Context) *e2eRig {
	t.Helper()
	ethRPC := env("E2E_ETH_RPC", defaultAnvilURL)
	subRPC := env("E2E_SUB_RPC", defaultBridgeURL)
	operatorKey := env("E2E_OPERATOR_KEY", defaultOperatorKey)

	t.Logf("e2e: connecting to anvil at %s", ethRPC)
	eth, err := ethclient.DialContext(ctx, ethRPC)
	if err != nil {
		t.Fatalf("e2e: dial anvil: %v", err)
	}
	t.Cleanup(eth.Close)

	chainID, err := eth.ChainID(ctx)
	if err != nil {
		t.Fatalf("e2e: anvil chain id: %v", err)
	}
	t.Logf("e2e: anvil chain id %s", chainID)

	t.Logf("e2e: connecting to bridgechain at %s", subRPC)
	sub, err := substrate.Dial(ctx, subRPC)
	if err != nil {
		t.Fatalf("e2e: dial bridgechain: %v", err)
	}
	t.Cleanup(func() { sub.Close() })

	vset, err := sub.GetBeefyValidatorSet(ctx)
	if err != nil {
		t.Fatalf("e2e: validator set: %v", err)
	}
	addrs, err := validators.EthAddressesFromBeefyPubkeys(vset.Validators)
	if err != nil {
		t.Fatalf("e2e: derive addresses: %v", err)
	}
	tree, err := validators.New(addrs)
	if err != nil {
		t.Fatalf("e2e: validator tree: %v", err)
	}
	root := tree.Root()
	t.Logf("e2e: validator set id=%d len=%d root=%x",
		vset.ID, len(vset.Validators), root)

	priv, err := crypto.HexToECDSA(operatorKey)
	if err != nil {
		t.Fatalf("e2e: parse operator key: %v", err)
	}
	auth, err := bind.NewKeyedTransactorWithChainID(priv, chainID)
	if err != nil {
		t.Fatalf("e2e: build transactor: %v", err)
	}
	auth.Context = ctx

	initial := bindings.BeefyClientValidatorSet{
		Id:     big.NewInt(int64(vset.ID)),
		Length: big.NewInt(int64(len(addrs))),
		Root:   root,
	}
	next := bindings.BeefyClientValidatorSet{
		Id:     big.NewInt(int64(vset.ID + 1)),
		Length: big.NewInt(int64(len(addrs))),
		Root:   root, // single-validator dev chain doesn't rotate
	}

	clientAddr, deployTx, client, err := bindings.DeployBeefyClient(
		auth, eth,
		big.NewInt(2),   // randaoCommitDelay
		big.NewInt(100), // randaoCommitExpiration
		big.NewInt(1),   // minNumRequiredSignatures
		big.NewInt(1),   // fiatShamirRequiredSignatures
		uint64(0),       // initialBeefyBlock
		initial,
		next,
	)
	if err != nil {
		t.Fatalf("e2e: deploy BeefyClient: %v", err)
	}
	if _, err := bind.WaitMined(ctx, eth, deployTx); err != nil {
		t.Fatalf("e2e: wait deploy: %v", err)
	}
	t.Logf("e2e: BeefyClient deployed at %s", clientAddr.Hex())

	subID, stream, err := sub.Subscribe(ctx,
		"beefy_subscribeJustifications", "beefy_unsubscribeJustifications")
	if err != nil {
		t.Fatalf("e2e: subscribe: %v", err)
	}
	t.Logf("e2e: subscription %s", subID)

	driver := &Driver{
		Beefy:        client,
		Chain:        &EthClientChain{C: eth},
		TxOpts:       auth,
		RandaoDelay:  2,
		PollInterval: 200 * time.Millisecond,
	}
	return &e2eRig{
		ctx:        ctx,
		t:          t,
		eth:        eth,
		sub:        sub,
		tree:       tree,
		client:     client,
		clientAddr: clientAddr,
		driver:     driver,
		stream:     stream,
	}
}

// relayOne reads the next BEEFY justification whose block number is
// strictly greater than `minBlock`, fetches its MMR proof, and drives
// the full commit-reveal cycle. Returns the commitment that was relayed
// for downstream assertions.
//
// Filtering by `minBlock` matters because the contract rejects stale
// commitments (`StaleCommitment` revert) — once we relay block N the
// next must be block > N.
func (r *e2eRig) relayOne(minBlock uint32) *beefy.SignedCommitment {
	r.t.Helper()

	var commit *beefy.SignedCommitment
	for {
		c, err := awaitJustification(r.ctx, r.stream)
		if err != nil {
			r.t.Fatalf("e2e: await justification: %v", err)
		}
		if c.Commitment.BlockNumber > minBlock {
			commit = c
			break
		}
		r.t.Logf("e2e: skipping stale commitment block=%d (waiting for > %d)",
			c.Commitment.BlockNumber, minBlock)
	}
	r.t.Logf("e2e: got commitment block=%d set_id=%d sigs=%d",
		commit.Commitment.BlockNumber,
		commit.Commitment.ValidatorSetID,
		commit.SignatureCount())

	// pallet-mmr appends a leaf for block N-1 during on_initialize(N), so
	// the leaf for block-number K lives at MMR index K-1.
	leafIdx := uint32(commit.Commitment.BlockNumber - 1)
	proofResp, err := r.sub.GenerateMmrProof(r.ctx, []uint32{leafIdx}, nil, nil)
	if err != nil {
		r.t.Fatalf("e2e: mmr_generateProof: %v", err)
	}
	leaves, err := mmr.DecodeLeaves(proofResp.Leaves)
	if err != nil {
		r.t.Fatalf("e2e: decode leaves: %v", err)
	}
	if len(leaves) != 1 {
		r.t.Fatalf("e2e: expected 1 leaf, got %d", len(leaves))
	}
	mmrProof, err := mmr.DecodeProof(proofResp.Proof)
	if err != nil {
		r.t.Fatalf("e2e: decode proof: %v", err)
	}
	simplified, err := mmr.Simplify(
		mmrProof.LeafIndices[0], mmrProof.LeafCount, mmrProof.Items)
	if err != nil {
		r.t.Fatalf("e2e: simplify proof: %v", err)
	}
	r.t.Logf("e2e: leaf parent=%d, proof items=%d order=%d",
		leaves[0].ParentNumber, len(simplified.Items), simplified.Order)

	if err := r.driver.Relay(r.ctx, commit, r.tree, leaves[0],
		simplified.Items, simplified.Order); err != nil {
		r.t.Fatalf("e2e: driver.Relay: %v", err)
	}
	return commit
}

// assertOnChainState confirms BeefyClient's stored state matches `commit`.
func (r *e2eRig) assertOnChainState(commit *beefy.SignedCommitment) {
	r.t.Helper()

	var wantRoot [32]byte
	for _, item := range commit.Commitment.Payload {
		if item.ID == [2]byte{'m', 'h'} {
			copy(wantRoot[:], item.Data)
			break
		}
	}
	gotRoot, err := r.client.LatestMMRRoot(&bind.CallOpts{Context: r.ctx})
	if err != nil {
		r.t.Fatalf("e2e: latestMMRRoot: %v", err)
	}
	if gotRoot != wantRoot {
		r.t.Errorf("e2e: latestMMRRoot mismatch\ngot:  %x\nwant: %x", gotRoot, wantRoot)
	}
	r.t.Logf("e2e: ✅ BeefyClient.latestMMRRoot = %x", gotRoot)

	gotBlock, err := r.client.LatestBeefyBlock(&bind.CallOpts{Context: r.ctx})
	if err != nil {
		r.t.Fatalf("e2e: latestBeefyBlock: %v", err)
	}
	if uint32(gotBlock) != commit.Commitment.BlockNumber {
		r.t.Errorf("e2e: latestBeefyBlock: got %d, want %d",
			gotBlock, commit.Commitment.BlockNumber)
	}
	r.t.Logf("e2e: ✅ BeefyClient.latestBeefyBlock = %d", gotBlock)
}

func TestE2E_BeefyRelay_singleValidator(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 4*time.Minute)
	defer cancel()
	rig := setupE2E(t, ctx)

	commit := rig.relayOne(0)
	rig.assertOnChainState(commit)
}

// TestE2E_BeefyRelay_consecutiveCommitments proves the bridge advances
// state durably — relays two commitments back-to-back and checks that
// `latestBeefyBlock` moves strictly forward each time.
//
// This is the gap between "works once" and "works as a bridge"; a one-shot
// success doesn't prove the contract's per-commitment bookkeeping
// (signature-usage counters, ticket lifecycle, stale-commitment check)
// is wired correctly.
func TestE2E_BeefyRelay_consecutiveCommitments(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 6*time.Minute)
	defer cancel()
	rig := setupE2E(t, ctx)

	first := rig.relayOne(0)
	rig.assertOnChainState(first)
	t.Logf("e2e: ─── first commitment relayed (block %d) ───",
		first.Commitment.BlockNumber)

	// Force the second commitment to be from a strictly later substrate
	// block; otherwise the contract would reject as StaleCommitment.
	second := rig.relayOne(first.Commitment.BlockNumber)
	rig.assertOnChainState(second)
	t.Logf("e2e: ─── second commitment relayed (block %d) ───",
		second.Commitment.BlockNumber)

	if second.Commitment.BlockNumber <= first.Commitment.BlockNumber {
		t.Errorf("e2e: second commitment block %d not strictly later than first %d",
			second.Commitment.BlockNumber, first.Commitment.BlockNumber)
	}
}

// awaitJustification reads one notification from `stream`, decodes it,
// and returns the SignedCommitment. Bails out on context cancellation.
func awaitJustification(
	ctx context.Context, stream <-chan json.RawMessage,
) (*beefy.SignedCommitment, error) {
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case raw, ok := <-stream:
		if !ok {
			return nil, fmt.Errorf("subscription closed")
		}
		var hexStr string
		if err := json.Unmarshal(raw, &hexStr); err != nil {
			return nil, fmt.Errorf("notification not a string: %w", err)
		}
		if len(hexStr) < 2 || hexStr[:2] != "0x" {
			return nil, fmt.Errorf("notification not hex-prefixed: %s", hexStr)
		}
		bytes, err := hex.DecodeString(hexStr[2:])
		if err != nil {
			return nil, fmt.Errorf("hex decode: %w", err)
		}
		return beefy.DecodeSignedCommitment(bytes)
	}
}
