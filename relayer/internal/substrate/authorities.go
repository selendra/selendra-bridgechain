package substrate

import (
	"context"
	"fmt"

	"github.com/nathselendra/bridgechain/relayer/internal/scale"
	"github.com/nathselendra/bridgechain/relayer/internal/validators"
)

// BeefyValidatorSet is the runtime view of the current BEEFY authority
// set, mirroring `sp_consensus_beefy::ValidatorSet<AuthorityId>`.
//
//   - `Validators` is the ordered list of 33-byte compressed secp256k1
//     pubkeys (`sp_consensus_beefy::ecdsa_crypto::Public`).
//   - `ID` is the rotation counter — the same value embedded in every
//     signed commitment's `validatorSetID` field.
//
// Order matters: validator `i` in this list is validator at index `i`
// in the Merkle tree the runtime publishes via `pallet-beefy-mmr`.
type BeefyValidatorSet struct {
	Validators []validators.BeefyPubkey
	ID         uint64
}

// GetBeefyValidatorSet invokes `BeefyApi_validator_set()` on the runtime
// and returns the decoded current authority set.
//
// The runtime API returns `Option<ValidatorSet<AuthorityId>>`; in
// practice on a live BEEFY chain it's always `Some`. We treat `None` as
// an error since the relayer can't operate without one.
func (c *Client) GetBeefyValidatorSet(ctx context.Context) (*BeefyValidatorSet, error) {
	raw, err := c.StateCall(ctx, "BeefyApi_validator_set", nil, nil)
	if err != nil {
		return nil, err
	}
	r := scale.NewReader(raw)
	tag, err := r.U8()
	if err != nil {
		return nil, fmt.Errorf("validator_set: option tag: %w", err)
	}
	switch tag {
	case 0x00:
		return nil, fmt.Errorf("validator_set: runtime returned None — BEEFY not active yet?")
	case 0x01:
		// fall through
	default:
		return nil, fmt.Errorf("validator_set: invalid Option tag 0x%02x", tag)
	}

	n, err := r.Compact()
	if err != nil {
		return nil, fmt.Errorf("validator_set: validators length: %w", err)
	}
	keys := make([]validators.BeefyPubkey, n)
	for i := uint64(0); i < n; i++ {
		if _, err := r.Read(keys[i][:]); err != nil {
			return nil, fmt.Errorf("validator_set: validator[%d]: %w", i, err)
		}
	}
	id, err := r.U64()
	if err != nil {
		return nil, fmt.Errorf("validator_set: id: %w", err)
	}
	return &BeefyValidatorSet{Validators: keys, ID: id}, nil
}
