package bitfield

import (
	"math/big"
	"reflect"
	"testing"
)

func TestContainerLength(t *testing.T) {
	cases := []struct {
		length int
		want   int
	}{
		{0, 0},
		{1, 1},
		{255, 1},
		{256, 1},
		{257, 2},
		{512, 2},
		{513, 3},
	}
	for _, c := range cases {
		if got := ContainerLength(c.length); got != c.want {
			t.Errorf("ContainerLength(%d) = %d, want %d", c.length, got, c.want)
		}
	}
}

func TestSetAndIsSet_LSBfirst(t *testing.T) {
	bf := New(300)
	// Set bit 0 → element 0, LSB.
	Set(bf, 0)
	if bf[0].Bit(0) != 1 {
		t.Errorf("bit 0: expected LSB of word 0 to be set, got %s", bf[0].Text(16))
	}
	// Set bit 7 → element 0, bit 7.
	Set(bf, 7)
	if bf[0].Bit(7) != 1 {
		t.Errorf("bit 7: not set")
	}
	// Set bit 256 → element 1, bit 0.
	Set(bf, 256)
	if bf[1].Bit(0) != 1 {
		t.Errorf("bit 256: expected LSB of word 1 to be set, got %s", bf[1].Text(16))
	}
	// IsSet matches.
	for _, i := range []int{0, 7, 256} {
		if !IsSet(bf, i) {
			t.Errorf("IsSet(%d) = false, want true", i)
		}
	}
	for _, i := range []int{1, 2, 255, 257} {
		if IsSet(bf, i) {
			t.Errorf("IsSet(%d) = true, want false", i)
		}
	}
}

func TestFrom_rejectsOutOfRange(t *testing.T) {
	if _, err := From([]int{0, 1, 10}, 10); err == nil {
		t.Errorf("expected error when index == length")
	}
	if _, err := From([]int{-1}, 10); err == nil {
		t.Errorf("expected error for negative index")
	}
}

func TestFrom_paddingIsZero(t *testing.T) {
	// 257 validators, set bits 0 and 256. Padding starts at bit 257 within
	// word 1; word 1 should equal exactly 1 (only bit 0 set, bits 1..255
	// all zero — passes the contract's `validatePadding`).
	bf, err := From([]int{0, 256}, 257)
	if err != nil {
		t.Fatalf("from: %v", err)
	}
	if len(bf) != 2 {
		t.Fatalf("container len: got %d, want 2", len(bf))
	}
	if bf[1].Cmp(big.NewInt(1)) != 0 {
		t.Errorf("word 1: got %s, want 1", bf[1].Text(16))
	}
}

func TestIndices_roundtrip(t *testing.T) {
	want := []int{0, 1, 7, 8, 255, 256, 257, 511}
	bf, err := From(want, 512)
	if err != nil {
		t.Fatalf("from: %v", err)
	}
	got := Indices(bf)
	if !reflect.DeepEqual(got, want) {
		t.Errorf("indices:\ngot:  %v\nwant: %v", got, want)
	}
}

func TestCount_matchesIndicesLen(t *testing.T) {
	bf, err := From([]int{0, 5, 100, 255, 256, 511}, 512)
	if err != nil {
		t.Fatalf("from: %v", err)
	}
	if Count(bf) != 6 {
		t.Errorf("count: got %d, want 6", Count(bf))
	}
	if len(Indices(bf)) != Count(bf) {
		t.Errorf("count/indices mismatch")
	}
}
