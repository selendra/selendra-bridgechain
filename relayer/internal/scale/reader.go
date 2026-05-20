// Package scale is a minimal SCALE codec — just enough decoding to handle
// the BEEFY/MMR types this relayer needs to interpret.
//
// SCALE primitives:
//   - integers: little-endian, fixed-width
//   - Option<T>: 0x00 = None; 0x01 ++ T = Some
//   - Vec<T>: compact length-prefix ++ T encoded N times
//   - compact-int: see Reader.Compact for the bit layout
//
// This package intentionally does not depend on go-substrate-rpc-client to
// keep the dependency graph small.
package scale

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
)

// Reader is a cursor over SCALE-encoded bytes. All reads advance the cursor.
type Reader struct {
	buf []byte
	pos int
}

// NewReader wraps a byte slice. The slice is not copied.
func NewReader(buf []byte) *Reader { return &Reader{buf: buf} }

// Remaining returns the unread byte count.
func (r *Reader) Remaining() int { return len(r.buf) - r.pos }

// Read fills dst entirely or returns io.ErrUnexpectedEOF.
func (r *Reader) Read(dst []byte) (int, error) {
	if r.pos+len(dst) > len(r.buf) {
		return 0, io.ErrUnexpectedEOF
	}
	copy(dst, r.buf[r.pos:])
	r.pos += len(dst)
	return len(dst), nil
}

// U8 reads one byte.
func (r *Reader) U8() (uint8, error) {
	if r.pos >= len(r.buf) {
		return 0, io.ErrUnexpectedEOF
	}
	b := r.buf[r.pos]
	r.pos++
	return b, nil
}

// U32 reads a little-endian uint32 (4 bytes).
func (r *Reader) U32() (uint32, error) {
	if r.pos+4 > len(r.buf) {
		return 0, io.ErrUnexpectedEOF
	}
	v := binary.LittleEndian.Uint32(r.buf[r.pos:])
	r.pos += 4
	return v, nil
}

// U64 reads a little-endian uint64 (8 bytes).
func (r *Reader) U64() (uint64, error) {
	if r.pos+8 > len(r.buf) {
		return 0, io.ErrUnexpectedEOF
	}
	v := binary.LittleEndian.Uint64(r.buf[r.pos:])
	r.pos += 8
	return v, nil
}

// Bytes32 reads a fixed-width 32-byte value (H256 / MerkleRoot).
func (r *Reader) Bytes32() ([32]byte, error) {
	var out [32]byte
	_, err := r.Read(out[:])
	return out, err
}

// Compact reads a SCALE compact integer.
//
// The low two bits of the first byte are the mode:
//
//	0b00: single byte, value = byte >> 2  (range 0..63)
//	0b01: two bytes,   value = u16 >> 2   (range 64..16383)
//	0b10: four bytes,  value = u32 >> 2   (range 16384..2³⁰-1)
//	0b11: big-int — top six bits encode (n-4) where n is the byte length
//	      of the little-endian payload that follows.
func (r *Reader) Compact() (uint64, error) {
	first, err := r.U8()
	if err != nil {
		return 0, err
	}
	mode := first & 0b11
	switch mode {
	case 0b00:
		return uint64(first >> 2), nil
	case 0b01:
		next, err := r.U8()
		if err != nil {
			return 0, err
		}
		return (uint64(next) << 6) | uint64(first>>2), nil
	case 0b10:
		rest := make([]byte, 3)
		if _, err := r.Read(rest); err != nil {
			return 0, err
		}
		return (uint64(rest[0]) << 6) |
			(uint64(rest[1]) << 14) |
			(uint64(rest[2]) << 22) |
			uint64(first>>2), nil
	default: // 0b11
		extra := int(first>>2) + 4
		if extra > 8 {
			return 0, errors.New("scale: compact value larger than u64")
		}
		body := make([]byte, extra)
		if _, err := r.Read(body); err != nil {
			return 0, err
		}
		var v uint64
		for i, b := range body {
			v |= uint64(b) << (8 * i)
		}
		return v, nil
	}
}

// ByteSlice reads `compact(len) ++ bytes` — i.e., `Vec<u8>`.
func (r *Reader) ByteSlice() ([]byte, error) {
	n, err := r.Compact()
	if err != nil {
		return nil, fmt.Errorf("byte slice: length: %w", err)
	}
	if int(n) > r.Remaining() {
		return nil, fmt.Errorf("byte slice: claimed %d bytes but only %d remain", n, r.Remaining())
	}
	out := make([]byte, n)
	if _, err := r.Read(out); err != nil {
		return nil, fmt.Errorf("byte slice: body: %w", err)
	}
	return out, nil
}
