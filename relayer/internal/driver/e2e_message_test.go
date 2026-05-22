//go:build e2e

// End-to-end test of the full Substrate→Ethereum message-passing leg:
//
//  1. Inject a signed `BridgeOutbound::submit` extrinsic via the
//     tools/inject-msg helper. That queues a message at some block M.
//  2. Wait for a BEEFY commitment for block > M and relay it through
//     BeefyClient (drives the existing commit-reveal cycle).
//  3. Deploy Gateway, point it at the BeefyClient.
//  4. Fetch the message + its MMR/Merkle proofs from bridgechain.
//  5. Call Gateway.submitInbound with the bundle.
//  6. Assert the InboundMessageDispatched event fires for the same nonce
//     and matches the destination address inject-msg targeted.
//
// Run via `bash relayer/scripts/e2e.sh` — that harness ensures inject-msg
// is built and exports INJECT_MSG_BIN.

package driver

import (
	"bytes"
	"context"
	"encoding/hex"
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/nathselendra/bridgechain/relayer/internal/bindings"
	"github.com/nathselendra/bridgechain/relayer/internal/outbound"
)

const (
	// Fixed destination/payload used by the test. Destination is the
	// zero-prefixed sentinel `…beef` (chosen so a future negative-test
	// can swap it for an EOA we control and verify the call landed).
	testDestination = "0x000000000000000000000000000000000000beef"
	testPayload     = "0xfeedface"
)

// injectMessageResult is the parsed stdout of the inject-msg helper.
type injectMessageResult struct {
	BlockHash   [32]byte
	BlockNumber uint32
}

// runInjectMsg invokes the standalone inject-msg binary built from
// tools/inject-msg/. Returns the block in which the extrinsic was finalized
// so the test can derive the MMR leaf to retrieve.
func runInjectMsg(ctx context.Context, t *testing.T, rpcURL string) injectMessageResult {
	t.Helper()
	binPath := os.Getenv("INJECT_MSG_BIN")
	if binPath == "" {
		t.Skip("INJECT_MSG_BIN not set — run via scripts/e2e.sh or build " +
			"tools/inject-msg manually and export INJECT_MSG_BIN")
	}
	cmd := exec.CommandContext(ctx, binPath,
		"--rpc", rpcURL,
		"--destination", testDestination,
		"--payload", testPayload,
	)
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	if err := cmd.Run(); err != nil {
		t.Fatalf("inject-msg: %v\nstderr:\n%s", err, stderr.String())
	}

	res := injectMessageResult{}
	for _, line := range strings.Split(strings.TrimSpace(stdout.String()), "\n") {
		kv := strings.SplitN(line, "=", 2)
		if len(kv) != 2 {
			continue
		}
		switch kv[0] {
		case "block_hash":
			raw, err := hex.DecodeString(strings.TrimPrefix(kv[1], "0x"))
			if err != nil || len(raw) != 32 {
				t.Fatalf("inject-msg: bad block_hash %q: %v", kv[1], err)
			}
			copy(res.BlockHash[:], raw)
		case "block_number":
			n, err := strconv.ParseUint(kv[1], 10, 32)
			if err != nil {
				t.Fatalf("inject-msg: bad block_number %q: %v", kv[1], err)
			}
			res.BlockNumber = uint32(n)
		}
	}
	if res.BlockNumber == 0 {
		t.Fatalf("inject-msg: missing block_number in output:\n%s", stdout.String())
	}
	t.Logf("e2e: injected message at block %d (hash %x)",
		res.BlockNumber, res.BlockHash)
	return res
}

// TestE2E_FullMessagePassing exercises the entire Substrate→Ethereum
// message delivery path end-to-end against live bridgechain + Anvil.
func TestE2E_FullMessagePassing(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 6*time.Minute)
	defer cancel()

	rig := setupE2E(t, ctx)
	subRPC := env("E2E_SUB_RPC", defaultBridgeURL)

	// 1. Submit a real extrinsic queueing a message.
	injected := runInjectMsg(ctx, t, subRPC)
	msgBlock := injected.BlockNumber

	// 2. Drive at least one BEEFY commitment that strictly post-dates the
	//    message's block. Once relayed, BeefyClient.latestMMRRoot covers
	//    the leaf containing the message's commitment root.
	commit := rig.relayOne(msgBlock)
	if commit.Commitment.BlockNumber <= msgBlock {
		t.Fatalf("relayed commitment block %d does not exceed message block %d",
			commit.Commitment.BlockNumber, msgBlock)
	}
	t.Logf("e2e: relayed BEEFY commitment for block %d (message in block %d)",
		commit.Commitment.BlockNumber, msgBlock)

	// 3. Deploy Gateway pointing at the freshly-updated BeefyClient.
	gatewayAddr, deployTx, gateway, err := bindings.DeployGateway(
		rig.driver.TxOpts, rig.eth, rig.clientAddr)
	if err != nil {
		t.Fatalf("e2e: deploy Gateway: %v", err)
	}
	if _, err := bind.WaitMined(ctx, rig.eth, deployTx); err != nil {
		t.Fatalf("e2e: wait Gateway deploy: %v", err)
	}
	t.Logf("e2e: Gateway deployed at %s", gatewayAddr.Hex())

	// 4. Fetch the message + proofs.
	bestBlock := commit.Commitment.BlockNumber
	bundle, err := outbound.FetchMessageBundle(ctx, rig.sub,
		msgBlock, 0, &bestBlock, nil)
	if err != nil {
		t.Fatalf("e2e: fetch bundle: %v", err)
	}
	t.Logf("e2e: bundle msg.nonce=%d dest=%x payload=%x leaf.parent=%d",
		bundle.Message.Nonce, bundle.Message.Destination,
		bundle.Message.Payload, bundle.MmrLeaf.ParentNumber)

	// 5. Convert to calldata + submit.
	submission := BuildInboundSubmission(bundle)
	tx, err := gateway.SubmitInbound(rig.driver.TxOpts,
		submission.Message, submission.Leaf,
		submission.LeafProof, submission.MsgProof)
	if err != nil {
		t.Fatalf("e2e: gateway.submitInbound: %v", err)
	}
	receipt, err := bind.WaitMined(ctx, rig.eth, tx)
	if err != nil {
		t.Fatalf("e2e: wait submitInbound: %v", err)
	}
	if receipt.Status != types.ReceiptStatusSuccessful {
		t.Fatalf("e2e: submitInbound reverted (tx=%s)", tx.Hash().Hex())
	}
	t.Logf("e2e: submitInbound mined in block %d (gas %d)",
		receipt.BlockNumber, receipt.GasUsed)

	// 6. Verify the dispatched event fired with the right nonce.
	dispatched, err := findDispatchedEvent(gateway, receipt)
	if err != nil {
		t.Fatalf("e2e: find dispatched event: %v", err)
	}
	if dispatched.Nonce != bundle.Message.Nonce {
		t.Errorf("e2e: event nonce: got %d, want %d",
			dispatched.Nonce, bundle.Message.Nonce)
	}
	wantDest := mustHex20(t, testDestination)
	if dispatched.Destination != common.Address(wantDest) {
		t.Errorf("e2e: event destination: got %x, want %x",
			dispatched.Destination, wantDest)
	}
	// The destination is a precompile-free dead address, so a `.call()` to
	// it returns success=true with empty returndata (Solidity treats calls
	// to addresses with no code as successful no-ops).
	if !dispatched.Success {
		t.Errorf("e2e: event success=false (expected dispatch to no-code addr to succeed)")
	}
	t.Logf("e2e: ✅ InboundMessageDispatched(nonce=%d, dest=%x, ok=%v)",
		dispatched.Nonce, dispatched.Destination, dispatched.Success)
}

func mustHex20(t *testing.T, s string) (out [20]byte) {
	t.Helper()
	raw, err := hex.DecodeString(strings.TrimPrefix(s, "0x"))
	if err != nil || len(raw) != 20 {
		t.Fatalf("bad 20-byte hex %q: %v", s, err)
	}
	copy(out[:], raw)
	return
}

// findDispatchedEvent scans `receipt`'s logs for an InboundMessageDispatched
// event emitted by `gateway`. Returns an error if exactly one wasn't found.
func findDispatchedEvent(gateway *bindings.Gateway, receipt *types.Receipt) (
	*bindings.GatewayInboundMessageDispatched, error,
) {
	for _, log := range receipt.Logs {
		ev, err := gateway.ParseInboundMessageDispatched(*log)
		if err != nil {
			continue
		}
		return ev, nil
	}
	return nil, fmt.Errorf("InboundMessageDispatched not found in %d logs",
		len(receipt.Logs))
}
