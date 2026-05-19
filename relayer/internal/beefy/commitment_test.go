package beefy

import (
	"encoding/binary"
	"testing"
)

func TestDecodeSignedCommitment_roundtrip(t *testing.T) {
	// Hand-crafted V1 commitment with a single MMR-root payload item and two
	// signature slots — one present, one absent.
	var sig Signature
	for i := range sig {
		sig[i] = byte(i + 1)
	}

	mmrRoot := make([]byte, 32)
	for i := range mmrRoot {
		mmrRoot[i] = 0xab
	}

	encoded := []byte{0x00} // version
	// Payload: 1 item
	encoded = append(encoded, 0x04)             // compact(1) = (1 << 2) | 0 = 0x04
	encoded = append(encoded, 'm', 'h')         // payload id
	encoded = append(encoded, 0x80)             // compact(32) = (32 << 2) | 0 = 0x80
	encoded = append(encoded, mmrRoot...)       // 32-byte mmr root
	// block_number = 42 (u32 LE)
	bn := make([]byte, 4)
	binary.LittleEndian.PutUint32(bn, 42)
	encoded = append(encoded, bn...)
	// validator_set_id = 7 (u64 LE)
	vsid := make([]byte, 8)
	binary.LittleEndian.PutUint64(vsid, 7)
	encoded = append(encoded, vsid...)
	// signatures: 2 slots, [Some(sig), None]
	encoded = append(encoded, 0x08) // compact(2) = (2 << 2) | 0 = 0x08
	encoded = append(encoded, 0x01)
	encoded = append(encoded, sig[:]...)
	encoded = append(encoded, 0x00)

	got, err := DecodeSignedCommitment(encoded)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if got.Commitment.BlockNumber != 42 {
		t.Errorf("block_number: got %d, want 42", got.Commitment.BlockNumber)
	}
	if got.Commitment.ValidatorSetID != 7 {
		t.Errorf("validator_set_id: got %d, want 7", got.Commitment.ValidatorSetID)
	}
	if len(got.Commitment.Payload) != 1 {
		t.Fatalf("payload count: got %d, want 1", len(got.Commitment.Payload))
	}
	if got.Commitment.Payload[0].ID != ([2]byte{'m', 'h'}) {
		t.Errorf("payload id: got %v, want 'mh'", got.Commitment.Payload[0].ID)
	}
	if len(got.Commitment.Payload[0].Data) != 32 {
		t.Errorf("payload data len: got %d, want 32", len(got.Commitment.Payload[0].Data))
	}
	if len(got.Signatures) != 2 {
		t.Fatalf("signatures len: got %d, want 2", len(got.Signatures))
	}
	if got.Signatures[0] == nil || *got.Signatures[0] != sig {
		t.Errorf("signature[0]: missing or mismatched")
	}
	if got.Signatures[1] != nil {
		t.Errorf("signature[1]: should be nil")
	}
	if got.SignatureCount() != 1 {
		t.Errorf("signature count: got %d, want 1", got.SignatureCount())
	}
}

func TestDecodeSignedCommitment_rejectsBadVersion(t *testing.T) {
	_, err := DecodeSignedCommitment([]byte{0x01})
	if err == nil {
		t.Fatal("expected error on unsupported version")
	}
}

func TestReadCompact_modes(t *testing.T) {
	cases := []struct {
		bytes []byte
		want  uint64
	}{
		{[]byte{0x00}, 0},
		{[]byte{0xfc}, 63},
		{[]byte{0x01, 0x01}, 64},
		{[]byte{0xfd, 0xff}, 16383},
		{[]byte{0x02, 0x00, 0x01, 0x00}, 16384},
		{[]byte{0x03, 0x00, 0x00, 0x00, 0x40}, 1 << 30},
	}
	for _, c := range cases {
		got, err := newReader(c.bytes).readCompact()
		if err != nil {
			t.Errorf("decode %v: %v", c.bytes, err)
			continue
		}
		if got != c.want {
			t.Errorf("decode %v: got %d, want %d", c.bytes, got, c.want)
		}
	}
}
