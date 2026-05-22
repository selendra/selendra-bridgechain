//go:build integration

package substrate

import (
	"context"
	"os"
	"testing"
	"time"

	"github.com/nathselendra/bridgechain/relayer/internal/validators"
)

const dialTimeout = 30 * time.Second

func nodeRPC() string {
	if v := os.Getenv("BRIDGECHAIN_SUBSTRATE_RPC"); v != "" {
		return v
	}
	return "ws://127.0.0.1:9944"
}

// TestIntegration_GetBeefyValidatorSet calls BeefyApi_validator_set on a
// live dev node and converts the returned pubkeys to eth addresses. On
// --dev the only validator is Alice, so we expect exactly one entry.
func TestIntegration_GetBeefyValidatorSet(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), dialTimeout)
	defer cancel()
	cli, err := Dial(ctx, nodeRPC())
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	defer cli.Close()

	set, err := cli.GetBeefyValidatorSet(ctx)
	if err != nil {
		t.Fatalf("get validator set: %v", err)
	}
	if len(set.Validators) == 0 {
		t.Fatal("validator set is empty")
	}
	t.Logf("set_id=%d validators=%d first_pubkey=%x...",
		set.ID, len(set.Validators), set.Validators[0][:8])

	// Convert to eth addresses — must succeed for every key.
	addrs, err := validators.EthAddressesFromBeefyPubkeys(set.Validators)
	if err != nil {
		t.Fatalf("address derivation: %v", err)
	}
	for i, a := range addrs {
		t.Logf("validator[%d] = 0x%x", i, a)
		if a == ([20]byte{}) {
			t.Errorf("validator[%d]: derived zero address", i)
		}
	}
}
