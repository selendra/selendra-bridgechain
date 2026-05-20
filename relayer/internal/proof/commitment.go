// Package proof turns a Substrate-side BEEFY justification + validator
// address set into the inputs `BeefyClient` expects: the commitment hash
// (which validators signed) and a [`bindings.BeefyClientValidatorProof`]
// per signature.
//
// Why split this out: the Ethereum-side contract recovers the signer from
// `(commitmentHash, v, r, s)`, then verifies a Merkle inclusion proof
// over the validator-set tree. Both pieces are deterministic functions of
// the SignedCommitment + the public validator address list; bundling them
// in one place keeps the driver state machine focused on sequencing.
package proof

import (
	"encoding/binary"

	"github.com/nathselendra/bridgechain/relayer/internal/beefy"
	"golang.org/x/crypto/sha3"
)

// EncodeCommitment SCALE-encodes a [`beefy.Commitment`] in the exact byte
// layout that Substrate's `codec::Encode` derive produces — fields in
// declaration order: payload, block_number, validator_set_id.
//
// Mirrors `BeefyClient.encodeCommitment` in BeefyClient.sol; the two must
// produce byte-identical output or the signature recovery on-chain will
// fail.
func EncodeCommitment(c beefy.Commitment) []byte {
	out := encodePayload(c.Payload)
	var u32 [4]byte
	binary.LittleEndian.PutUint32(u32[:], c.BlockNumber)
	out = append(out, u32[:]...)
	var u64 [8]byte
	binary.LittleEndian.PutUint64(u64[:], c.ValidatorSetID)
	return append(out, u64[:]...)
}

// CommitmentHash is the value validators sign and that `submitInitial` /
// `submitFinal` recompute on chain: `keccak256(SCALE(commitment))`.
func CommitmentHash(c beefy.Commitment) [32]byte {
	h := sha3.NewLegacyKeccak256()
	h.Write(EncodeCommitment(c))
	var out [32]byte
	copy(out[:], h.Sum(nil))
	return out
}

func encodePayload(items []beefy.PayloadItem) []byte {
	out := compactEncode(uint64(len(items)))
	for _, item := range items {
		out = append(out, item.ID[:]...)
		out = append(out, compactEncode(uint64(len(item.Data)))...)
		out = append(out, item.Data...)
	}
	return out
}

// compactEncode is the SCALE compact-int encoder. The decoder lives in
// `internal/scale` — keep the inverse here local to the package to avoid
// dragging an encoding API into the decoder-only scale package for now.
func compactEncode(v uint64) []byte {
	switch {
	case v < 1<<6:
		return []byte{byte(v << 2)}
	case v < 1<<14:
		x := uint16(v<<2) | 0b01
		return []byte{byte(x), byte(x >> 8)}
	case v < 1<<30:
		x := uint32(v<<2) | 0b10
		return []byte{byte(x), byte(x >> 8), byte(x >> 16), byte(x >> 24)}
	}
	// big-int mode: 4..8 byte LE payload.
	body := []byte{}
	for v > 0 {
		body = append(body, byte(v))
		v >>= 8
	}
	header := byte(((len(body) - 4) << 2) | 0b11)
	return append([]byte{header}, body...)
}
