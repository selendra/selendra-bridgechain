// Package beefy implements the Substrate → Ethereum direction of the bridge.
//
// On every new BEEFY justification:
//
//  1. Decode the SignedCommitment (SCALE-encoded EncodedVersionedFinalityProof
//     v1, which wraps a Commitment + ECDSA signatures).
//  2. Fetch the corresponding MMR leaf + leaf proof from the node.
//  3. Drive the BeefyClient contract through commit-reveal (submitInitial →
//     commitPrevRandao → submitFinal) or Fiat-Shamir (single-call).
//
// In skeleton form: step 1 happens (raw bytes are logged), steps 2 and 3 are
// TODO. The point is to show the wire is alive.
package beefy

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log/slog"

	"github.com/nathselendra/bridgechain/relayer/internal/ethereum"
	"github.com/nathselendra/bridgechain/relayer/internal/substrate"
)

// Relay drives the Substrate-side commitment subscription and (eventually)
// the Ethereum-side submission.
type Relay struct {
	sub       *substrate.Client
	eth       *ethereum.Client
	beefyAddr string
}

// NewRelay wires the substrate + ethereum clients together. The Ethereum
// destination address may be empty in skeleton mode — submission is logged
// rather than sent.
func NewRelay(sub *substrate.Client, eth *ethereum.Client, beefyAddr string) *Relay {
	return &Relay{sub: sub, eth: eth, beefyAddr: beefyAddr}
}

// Run blocks until ctx is cancelled. It subscribes to BEEFY justifications
// and processes each one as it arrives.
func (r *Relay) Run(ctx context.Context) error {
	subID, stream, err := r.sub.Subscribe(ctx,
		"beefy_subscribeJustifications", "beefy_unsubscribeJustifications")
	if err != nil {
		return fmt.Errorf("beefy: subscribe: %w", err)
	}
	slog.Info("beefy: subscribed", "subscription", subID)

	for {
		select {
		case <-ctx.Done():
			return nil
		case raw, ok := <-stream:
			if !ok {
				return fmt.Errorf("beefy: subscription stream closed")
			}
			if err := r.handle(ctx, raw); err != nil {
				slog.Warn("beefy: skipping commitment", "err", err)
			}
		}
	}
}

// handle is intentionally tiny in the skeleton — it confirms the wire
// format, then logs what would happen next.
func (r *Relay) handle(ctx context.Context, raw json.RawMessage) error {
	// Notifications are hex-encoded SCALE bytes ("0x..."). The decoder lives in
	// commitment.go.
	var hexStr string
	if err := json.Unmarshal(raw, &hexStr); err != nil {
		return fmt.Errorf("decode hex string: %w", err)
	}
	if len(hexStr) < 2 || hexStr[:2] != "0x" {
		return fmt.Errorf("notification not hex-prefixed: %s", hexStr)
	}
	encoded, err := hex.DecodeString(hexStr[2:])
	if err != nil {
		return fmt.Errorf("decode hex body: %w", err)
	}

	commit, err := DecodeSignedCommitment(encoded)
	if err != nil {
		return fmt.Errorf("decode signed commitment: %w", err)
	}

	slog.Info("beefy: new justification",
		"block_number", commit.Commitment.BlockNumber,
		"validator_set_id", commit.Commitment.ValidatorSetID,
		"payload_items", len(commit.Commitment.Payload),
		"signature_slots", len(commit.Signatures),
		"signatures_present", commit.SignatureCount())

	// TODO: fetch MMR leaf + proof via mmr_generateProof, drive BeefyClient
	// through commit-reveal. Address parameter is plumbed but unused.
	if r.beefyAddr != "" {
		slog.Debug("beefy: would submit to client", "address", r.beefyAddr)
	}
	return nil
}
