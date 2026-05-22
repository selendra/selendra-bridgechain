package validators

import (
	"testing"

	ethcrypto "github.com/ethereum/go-ethereum/crypto"
)

func TestEthAddressFromBeefyPubkey_anvilKey(t *testing.T) {
	const hexKey = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
	key, err := ethcrypto.HexToECDSA(hexKey)
	if err != nil {
		t.Fatalf("hex key: %v", err)
	}
	want := ethcrypto.PubkeyToAddress(key.PublicKey)

	// CompressPubkey returns the 33-byte SEC1 compressed form — the same
	// shape BEEFY stores its authority keys in.
	compressed := ethcrypto.CompressPubkey(&key.PublicKey)
	if len(compressed) != 33 {
		t.Fatalf("compressed len: got %d, want 33", len(compressed))
	}
	var pk BeefyPubkey
	copy(pk[:], compressed)

	got, err := EthAddressFromBeefyPubkey(pk)
	if err != nil {
		t.Fatalf("address: %v", err)
	}
	if [20]byte(want) != got {
		t.Errorf("address: got %x, want %x", got, want.Bytes())
	}
}

func TestEthAddressFromBeefyPubkey_rejectsGarbage(t *testing.T) {
	// First byte of a valid compressed pubkey is 0x02 or 0x03; anything
	// else is malformed. Use 0xff to make sure DecompressPubkey errors.
	var pk BeefyPubkey
	for i := range pk {
		pk[i] = 0xff
	}
	if _, err := EthAddressFromBeefyPubkey(pk); err == nil {
		t.Fatal("expected error on malformed pubkey")
	}
}

func TestEthAddressesFromBeefyPubkeys_multiple(t *testing.T) {
	hexKeys := []string{
		"ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
		"59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
	}
	pubkeys := make([]BeefyPubkey, len(hexKeys))
	want := make([][20]byte, len(hexKeys))
	for i, hk := range hexKeys {
		k, _ := ethcrypto.HexToECDSA(hk)
		copy(pubkeys[i][:], ethcrypto.CompressPubkey(&k.PublicKey))
		copy(want[i][:], ethcrypto.PubkeyToAddress(k.PublicKey).Bytes())
	}

	got, err := EthAddressesFromBeefyPubkeys(pubkeys)
	if err != nil {
		t.Fatalf("addresses: %v", err)
	}
	for i := range got {
		if got[i] != want[i] {
			t.Errorf("addr[%d]: got %x, want %x", i, got[i], want[i])
		}
	}
}
