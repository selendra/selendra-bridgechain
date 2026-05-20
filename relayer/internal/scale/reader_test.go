package scale

import "testing"

func TestCompact_modes(t *testing.T) {
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
		// big-int mode: 5 bytes total (1 prefix + 4 payload), value = 2^30
		{[]byte{0x07, 0x00, 0x00, 0x00, 0x40, 0x00}, 1 << 30},
	}
	for _, c := range cases {
		got, err := NewReader(c.bytes).Compact()
		if err != nil {
			t.Errorf("decode %x: %v", c.bytes, err)
			continue
		}
		if got != c.want {
			t.Errorf("decode %x: got %d, want %d", c.bytes, got, c.want)
		}
	}
}

func TestByteSlice(t *testing.T) {
	r := NewReader([]byte{0x08, 0xaa, 0xbb}) // compact(2) ++ [0xaa, 0xbb]
	got, err := r.ByteSlice()
	if err != nil {
		t.Fatalf("byte slice: %v", err)
	}
	if len(got) != 2 || got[0] != 0xaa || got[1] != 0xbb {
		t.Errorf("byte slice: got %x, want [aa bb]", got)
	}
}

func TestU32_LE(t *testing.T) {
	r := NewReader([]byte{0x2a, 0x00, 0x00, 0x00})
	got, err := r.U32()
	if err != nil {
		t.Fatal(err)
	}
	if got != 42 {
		t.Errorf("u32: got %d, want 42", got)
	}
}

func TestU64_LE(t *testing.T) {
	r := NewReader([]byte{0x07, 0, 0, 0, 0, 0, 0, 0})
	got, err := r.U64()
	if err != nil {
		t.Fatal(err)
	}
	if got != 7 {
		t.Errorf("u64: got %d, want 7", got)
	}
}
