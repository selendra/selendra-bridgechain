package driver

import (
	"math/big"

	"github.com/ethereum/go-ethereum/common"
	"github.com/nathselendra/bridgechain/relayer/internal/bindings"
	"github.com/nathselendra/bridgechain/relayer/internal/outbound"
)

// InboundSubmission packages the four calldata structs Gateway.submitInbound
// consumes. Builds cleanly from an outbound.Bundle assembled by the relayer.
type InboundSubmission struct {
	Message   bindings.OutboundMessage
	Leaf      bindings.GatewayMmrLeaf
	LeafProof bindings.GatewayMmrLeafProof
	MsgProof  bindings.GatewayMessageProof
}

// BuildInboundSubmission converts an outbound.Bundle into the calldata shape
// the Gateway contract expects. Pure data shuffling — no I/O, no validation.
// All validation is on-chain in `Gateway.submitInbound`.
func BuildInboundSubmission(b *outbound.Bundle) InboundSubmission {
	leaf := bindings.GatewayMmrLeaf{
		Version:              b.MmrLeaf.Version,
		ParentNumber:         b.MmrLeaf.ParentNumber,
		ParentHash:           b.MmrLeaf.ParentHash,
		NextAuthoritySetID:   b.MmrLeaf.NextAuthoritySet.ID,
		NextAuthoritySetLen:  b.MmrLeaf.NextAuthoritySet.Len,
		NextAuthoritySetRoot: b.MmrLeaf.NextAuthoritySet.Root,
		LeafExtra:            b.MmrLeaf.LeafExtra,
	}

	leafProof := bindings.GatewayMmrLeafProof{
		Siblings: b.MmrProofSimplified.Items,
		Order:    new(big.Int).SetUint64(b.MmrProofSimplified.Order),
	}

	msgProof := bindings.GatewayMessageProof{
		Position: new(big.Int).SetUint64(uint64(b.MessageProof.LeafIndex)),
		Width:    new(big.Int).SetUint64(uint64(b.MessageProof.LeafCount)),
		Proof:    b.MessageProof.Items,
	}

	return InboundSubmission{
		Message: bindings.OutboundMessage{
			Nonce:       b.Message.Nonce,
			Destination: common.Address(b.Message.Destination),
			Payload:     b.Message.Payload,
		},
		Leaf:      leaf,
		LeafProof: leafProof,
		MsgProof:  msgProof,
	}
}
