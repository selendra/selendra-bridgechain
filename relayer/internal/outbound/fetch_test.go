package outbound

import (
	"bytes"
	"encoding/binary"
	"testing"
)

func TestDecodeMessages(t *testing.T) {
	// Vec<OutboundMessage> with one entry: nonce=1, destination=0xCAFE...,
	// payload=0xabcd
	dest := [20]byte{}
	for i := range dest {
		dest[i] = 0xCA
	}
	var buf bytes.Buffer
	buf.WriteByte(0x04) // compact(1)
	binary.Write(&buf, binary.LittleEndian, uint64(1))
	buf.Write(dest[:])
	buf.WriteByte(0x08)         // compact(2)
	buf.Write([]byte{0xab, 0xcd})

	got, err := decodeMessages(buf.Bytes())
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if len(got) != 1 {
		t.Fatalf("len: %d", len(got))
	}
	if got[0].Nonce != 1 {
		t.Errorf("nonce: %d", got[0].Nonce)
	}
	if got[0].Destination != dest {
		t.Errorf("destination mismatch")
	}
	if len(got[0].Payload) != 2 || got[0].Payload[0] != 0xab {
		t.Errorf("payload: %x", got[0].Payload)
	}
}

func TestDecodeMessageProofOption_None(t *testing.T) {
	got, err := decodeMessageProofOption([]byte{0x00})
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if got != nil {
		t.Errorf("expected nil for None")
	}
}

func TestDecodeMessageProofOption_Some(t *testing.T) {
	var root [32]byte
	for i := range root {
		root[i] = 0x11
	}
	leafBytes := []byte{0xaa, 0xbb, 0xcc}

	var buf bytes.Buffer
	buf.WriteByte(0x01) // Some
	buf.Write(root[:])
	buf.WriteByte(0x08) // compact(2) — 2 proof items
	item0 := [32]byte{}
	item1 := [32]byte{}
	for i := range item0 {
		item0[i] = 0x22
		item1[i] = 0x33
	}
	buf.Write(item0[:])
	buf.Write(item1[:])
	binary.Write(&buf, binary.LittleEndian, uint32(5)) // leaf_count
	binary.Write(&buf, binary.LittleEndian, uint32(2)) // leaf_index
	buf.WriteByte(0x0c)                                  // compact(3) — leaf len
	buf.Write(leafBytes)

	got, err := decodeMessageProofOption(buf.Bytes())
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if got == nil {
		t.Fatal("got nil")
	}
	if got.Root != root {
		t.Errorf("root mismatch")
	}
	if len(got.Items) != 2 {
		t.Errorf("items: %d", len(got.Items))
	}
	if got.Items[0] != item0 || got.Items[1] != item1 {
		t.Errorf("items contents")
	}
	if got.LeafCount != 5 || got.LeafIndex != 2 {
		t.Errorf("leaf count/index: (%d, %d)", got.LeafCount, got.LeafIndex)
	}
	if !bytes.Equal(got.LeafBytes, leafBytes) {
		t.Errorf("leaf bytes: %x", got.LeafBytes)
	}
}
