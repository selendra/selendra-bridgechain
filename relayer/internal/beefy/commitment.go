package beefy

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
)

// PayloadID is the 2-byte identifier used by Substrate to tag a payload item.
//
// The MMR root payload carries id `b"mh"` — see
// substrate/primitives/consensus/beefy/src/payload.rs.
type PayloadID [2]byte

// PayloadItem is one entry in the BEEFY commitment payload.
type PayloadItem struct {
	ID   PayloadID
	Data []byte
}

// Commitment is the payload that BEEFY validators sign. Generic over the
// block-number type, fixed to u32 here for bridgechain.
type Commitment struct {
	Payload        []PayloadItem
	BlockNumber    uint32
	ValidatorSetID uint64
}

// Signature is a 65-byte secp256k1 ECDSA signature (r ‖ s ‖ v).
type Signature [65]byte

// SignedCommitment is the parsed `VersionedFinalityProof::V1`.
//
// `Signatures` has one slot per active validator. `nil` means that validator
// did not sign (encoded as `Option::None` on the Substrate side).
type SignedCommitment struct {
	Commitment Commitment
	Signatures []*Signature
}

// SignatureCount returns the number of non-nil signature slots.
func (s *SignedCommitment) SignatureCount() int {
	n := 0
	for _, sig := range s.Signatures {
		if sig != nil {
			n++
		}
	}
	return n
}

// DecodeSignedCommitment parses a SCALE-encoded
// `EncodedVersionedFinalityProof` (the over-the-wire form of a BEEFY
// justification).
//
// Wire format (see `CompactSignedCommitment` in
// substrate/primitives/consensus/beefy/src/commitment.rs — SignedCommitment's
// custom Encode/Decode packs Vec<Option<Signature>> into a bitfield +
// only the present signatures):
//
//	u8 variant tag (must be 1 for V1 — `#[codec(index = 1)]`)
//	Commitment {
//	    Vec<(PayloadID, Vec<u8>)>     // compact length-prefixed
//	    u32 block_number (LE)
//	    u64 validator_set_id (LE)
//	}
//	Vec<u8> signatures_from          // compact-prefixed bitfield, one bit
//	                                  // per validator, MSB-first within byte
//	u32     validator_set_len (LE)   // significant bit count
//	Vec<Signature> signatures_compact // compact-prefixed, only present sigs
func DecodeSignedCommitment(buf []byte) (*SignedCommitment, error) {
	r := newReader(buf)

	version, err := r.readU8()
	if err != nil {
		return nil, fmt.Errorf("version: %w", err)
	}
	if version != 1 {
		return nil, fmt.Errorf("unsupported VersionedFinalityProof variant %d", version)
	}

	payloadLen, err := r.readCompact()
	if err != nil {
		return nil, fmt.Errorf("payload length: %w", err)
	}
	payload := make([]PayloadItem, 0, payloadLen)
	for i := uint64(0); i < payloadLen; i++ {
		var item PayloadItem
		if _, err := r.read(item.ID[:]); err != nil {
			return nil, fmt.Errorf("payload[%d].id: %w", i, err)
		}
		dataLen, err := r.readCompact()
		if err != nil {
			return nil, fmt.Errorf("payload[%d].data length: %w", i, err)
		}
		item.Data = make([]byte, dataLen)
		if _, err := r.read(item.Data); err != nil {
			return nil, fmt.Errorf("payload[%d].data: %w", i, err)
		}
		payload = append(payload, item)
	}

	blockNumber, err := r.readU32()
	if err != nil {
		return nil, fmt.Errorf("block_number: %w", err)
	}
	validatorSetID, err := r.readU64()
	if err != nil {
		return nil, fmt.Errorf("validator_set_id: %w", err)
	}

	bitfieldLen, err := r.readCompact()
	if err != nil {
		return nil, fmt.Errorf("signatures_from length: %w", err)
	}
	bitfield := make([]byte, bitfieldLen)
	if _, err := r.read(bitfield); err != nil {
		return nil, fmt.Errorf("signatures_from: %w", err)
	}

	validatorSetLen, err := r.readU32()
	if err != nil {
		return nil, fmt.Errorf("validator_set_len: %w", err)
	}

	compactCount, err := r.readCompact()
	if err != nil {
		return nil, fmt.Errorf("signatures_compact length: %w", err)
	}
	compactSigs := make([]Signature, compactCount)
	for i := uint64(0); i < compactCount; i++ {
		if _, err := r.read(compactSigs[i][:]); err != nil {
			return nil, fmt.Errorf("signatures_compact[%d]: %w", i, err)
		}
	}

	// Reconstruct Vec<Option<Signature>>: walk the bitfield MSB-first, and
	// each set bit consumes the next signature from signatures_compact.
	signatures := make([]*Signature, validatorSetLen)
	sigIdx := 0
	for i := uint32(0); i < validatorSetLen; i++ {
		byteIdx := int(i / 8)
		bitOff := i % 8
		if byteIdx >= len(bitfield) {
			return nil, fmt.Errorf("signatures_from: bitfield too short for validator %d", i)
		}
		bit := (bitfield[byteIdx] >> (7 - bitOff)) & 1
		if bit == 1 {
			if sigIdx >= len(compactSigs) {
				return nil, fmt.Errorf("signatures_compact: ran out at validator %d", i)
			}
			sig := compactSigs[sigIdx]
			signatures[i] = &sig
			sigIdx++
		}
	}
	if sigIdx != len(compactSigs) {
		return nil, fmt.Errorf("signatures_compact: %d unused (consumed %d / %d)",
			len(compactSigs)-sigIdx, sigIdx, len(compactSigs))
	}

	return &SignedCommitment{
		Commitment: Commitment{
			Payload:        payload,
			BlockNumber:    blockNumber,
			ValidatorSetID: validatorSetID,
		},
		Signatures: signatures,
	}, nil
}

// reader is a tiny cursor over the encoded bytes. SCALE primitives are all
// little-endian and length-prefixed via compact-int, so the surface area
// needed is small.
type reader struct {
	buf []byte
	pos int
}

func newReader(buf []byte) *reader { return &reader{buf: buf} }

func (r *reader) read(dst []byte) (int, error) {
	if r.pos+len(dst) > len(r.buf) {
		return 0, io.ErrUnexpectedEOF
	}
	copy(dst, r.buf[r.pos:])
	r.pos += len(dst)
	return len(dst), nil
}

func (r *reader) readU8() (uint8, error) {
	if r.pos >= len(r.buf) {
		return 0, io.ErrUnexpectedEOF
	}
	b := r.buf[r.pos]
	r.pos++
	return b, nil
}

func (r *reader) readU32() (uint32, error) {
	if r.pos+4 > len(r.buf) {
		return 0, io.ErrUnexpectedEOF
	}
	v := binary.LittleEndian.Uint32(r.buf[r.pos:])
	r.pos += 4
	return v, nil
}

func (r *reader) readU64() (uint64, error) {
	if r.pos+8 > len(r.buf) {
		return 0, io.ErrUnexpectedEOF
	}
	v := binary.LittleEndian.Uint64(r.buf[r.pos:])
	r.pos += 8
	return v, nil
}

// readCompact reads a SCALE compact integer.
//
// The low two bits of the first byte are the mode:
//
//	0b00: single byte, value = byte >> 2 (range 0..63)
//	0b01: two bytes,   value = u16 >> 2  (range 64..16383)
//	0b10: four bytes,  value = u32 >> 2  (range 16384..2³⁰-1)
//	0b11: big-int — top six bits encode (n-4) where n is the length of the
//	      little-endian value following.
func (r *reader) readCompact() (uint64, error) {
	first, err := r.readU8()
	if err != nil {
		return 0, err
	}
	mode := first & 0b11
	switch mode {
	case 0b00:
		return uint64(first >> 2), nil
	case 0b01:
		next, err := r.readU8()
		if err != nil {
			return 0, err
		}
		return (uint64(next) << 6) | uint64(first>>2), nil
	case 0b10:
		rest := make([]byte, 3)
		if _, err := r.read(rest); err != nil {
			return 0, err
		}
		return (uint64(rest[0]) << 6) |
			(uint64(rest[1]) << 14) |
			(uint64(rest[2]) << 22) |
			uint64(first>>2), nil
	default: // 0b11
		extra := int(first>>2) + 4
		if extra > 8 {
			return 0, errors.New("compact: encoded value larger than u64")
		}
		body := make([]byte, extra)
		if _, err := r.read(body); err != nil {
			return 0, err
		}
		var v uint64
		for i, b := range body {
			v |= uint64(b) << (8 * i)
		}
		return v, nil
	}
}
