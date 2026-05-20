package mmr

import (
	"bytes"
	"encoding/binary"
	"encoding/hex"
	"testing"

	"github.com/nathselendra/bridgechain/relayer/internal/scale"
	"golang.org/x/crypto/sha3"
)

// buildLeaf produces the SCALE encoding of a bridgechain MMR leaf with the
// supplied field values. Used by multiple tests.
func buildLeaf(version uint8, parentNumber uint32, parentHash [32]byte,
	setID uint64, setLen uint32, setRoot [32]byte, leafExtra [32]byte) []byte {
	var buf bytes.Buffer
	buf.WriteByte(version)
	binary.Write(&buf, binary.LittleEndian, parentNumber)
	buf.Write(parentHash[:])
	binary.Write(&buf, binary.LittleEndian, setID)
	binary.Write(&buf, binary.LittleEndian, setLen)
	buf.Write(setRoot[:])
	buf.Write(leafExtra[:])
	return buf.Bytes()
}

func TestDecodeLeaf_roundtrip(t *testing.T) {
	parentHash := repeatByte(0xaa)
	setRoot := repeatByte(0xbb)
	leafExtra := repeatByte(0xcc)
	encoded := buildLeaf(0, 42, parentHash, 7, 4, setRoot, leafExtra)

	leaf, err := DecodeLeaf(scale.NewReader(encoded))
	if err != nil {
		t.Fatalf("decode leaf: %v", err)
	}
	if leaf.Version != 0 || leaf.ParentNumber != 42 {
		t.Errorf("version/parent: got (%d, %d), want (0, 42)", leaf.Version, leaf.ParentNumber)
	}
	if leaf.ParentHash != parentHash {
		t.Errorf("parent hash mismatch")
	}
	if leaf.NextAuthoritySet.ID != 7 || leaf.NextAuthoritySet.Len != 4 {
		t.Errorf("authority set fields wrong")
	}
	if leaf.NextAuthoritySet.Root != setRoot {
		t.Errorf("authority root mismatch")
	}
	if leaf.LeafExtra != leafExtra {
		t.Errorf("leaf_extra mismatch")
	}
}

func TestDecodeLeaves_vec(t *testing.T) {
	a := buildLeaf(0, 1, repeatByte(0x11), 0, 4, repeatByte(0x22), repeatByte(0x33))
	b := buildLeaf(0, 2, repeatByte(0x44), 0, 4, repeatByte(0x55), repeatByte(0x66))

	// `mmr_generateProof` returns Vec<EncodableOpaqueLeaf>, where each
	// EncodableOpaqueLeaf is itself a length-prefixed Vec<u8>. So the wire
	// form is: compact(2) ++ compact(len(a)) ++ a ++ compact(len(b)) ++ b.
	encoded := []byte{0x08}
	encoded = append(encoded, compactEncode(uint64(len(a)))...)
	encoded = append(encoded, a...)
	encoded = append(encoded, compactEncode(uint64(len(b)))...)
	encoded = append(encoded, b...)

	leaves, err := DecodeLeaves(encoded)
	if err != nil {
		t.Fatalf("decode leaves: %v", err)
	}
	if len(leaves) != 2 {
		t.Fatalf("len: got %d, want 2", len(leaves))
	}
	if leaves[0].ParentNumber != 1 || leaves[1].ParentNumber != 2 {
		t.Errorf("parent numbers: %v", []uint32{leaves[0].ParentNumber, leaves[1].ParentNumber})
	}
}

func compactEncode(v uint64) []byte {
	switch {
	case v < 1<<6:
		return []byte{byte(v << 2)}
	case v < 1<<14:
		x := uint16(v<<2) | 1
		return []byte{byte(x), byte(x >> 8)}
	case v < 1<<30:
		x := uint32(v<<2) | 2
		return []byte{byte(x), byte(x >> 8), byte(x >> 16), byte(x >> 24)}
	}
	panic("compactEncode: value too large for test helper")
}

func TestDecodeProof(t *testing.T) {
	// leaf_indices = [3], leaf_count = 10, items = [0x77x32, 0x88x32]
	var buf bytes.Buffer
	buf.WriteByte(0x04) // compact(1)
	binary.Write(&buf, binary.LittleEndian, uint64(3))
	binary.Write(&buf, binary.LittleEndian, uint64(10))
	buf.WriteByte(0x08) // compact(2)
	buf.Write(repeatByteSlice(0x77))
	buf.Write(repeatByteSlice(0x88))

	proof, err := DecodeProof(buf.Bytes())
	if err != nil {
		t.Fatalf("decode proof: %v", err)
	}
	if len(proof.LeafIndices) != 1 || proof.LeafIndices[0] != 3 {
		t.Errorf("leaf_indices: %v", proof.LeafIndices)
	}
	if proof.LeafCount != 10 {
		t.Errorf("leaf_count: %d", proof.LeafCount)
	}
	if len(proof.Items) != 2 {
		t.Fatalf("items len: %d", len(proof.Items))
	}
	if proof.Items[0] != repeatByte(0x77) || proof.Items[1] != repeatByte(0x88) {
		t.Errorf("items mismatch")
	}
}

func TestLeafHash_matchesSolidity(t *testing.T) {
	// Same fixture as contracts/test/Gateway.t.sol::test_hashMmrLeafMatchesScaleEncoding:
	//   version=0, parentNumber=42, parentHash=0xaa*32,
	//   nextAuthoritySetID=7, nextAuthoritySetLen=4, nextAuthoritySetRoot=0xbb*32,
	//   leafExtra=0xcc*32
	// Hand-rolled expected: keccak256(SCALE(...))
	expected := keccakHex(t,
		"00"+
			"2a000000"+
			hexRepeat("aa", 32)+
			"0700000000000000"+
			"04000000"+
			hexRepeat("bb", 32)+
			hexRepeat("cc", 32))

	leaf := Leaf{
		Version:       0,
		ParentNumber:  42,
		ParentHash:    fillByte(0xaa),
		NextAuthoritySet: AuthoritySet{
			ID:   7,
			Len:  4,
			Root: fillByte(0xbb),
		},
		LeafExtra: fillByte(0xcc),
	}
	got := leaf.LeafHash()
	if hex.EncodeToString(got[:]) != expected {
		t.Errorf("leaf hash mismatch\n  got:  %x\n  want: %s", got, expected)
	}
}

func repeatByte(b byte) [32]byte {
	var out [32]byte
	for i := range out {
		out[i] = b
	}
	return out
}

func repeatByteSlice(b byte) []byte {
	out := make([]byte, 32)
	for i := range out {
		out[i] = b
	}
	return out
}

func fillByte(b byte) [32]byte { return repeatByte(b) }

func hexRepeat(s string, n int) string {
	out := ""
	for i := 0; i < n; i++ {
		out += s
	}
	return out
}

func keccakHex(t *testing.T, hexStr string) string {
	t.Helper()
	b, err := hex.DecodeString(hexStr)
	if err != nil {
		t.Fatalf("hex decode: %v", err)
	}
	h := sha3.NewLegacyKeccak256()
	h.Write(b)
	return hex.EncodeToString(h.Sum(nil))
}
