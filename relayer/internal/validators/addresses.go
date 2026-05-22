package validators

import (
	"fmt"

	ethcrypto "github.com/ethereum/go-ethereum/crypto"
)

// BeefyPubkey is the 33-byte compressed secp256k1 public key as it
// appears on the Substrate side (`sp_consensus_beefy::ecdsa_crypto::Public`
// is a wrapper around `[u8; 33]`).
type BeefyPubkey [33]byte

// EthAddressFromBeefyPubkey decompresses a secp256k1 compressed pubkey
// and returns the corresponding 20-byte Ethereum address.
//
// This is the conversion `pallet-beefy-mmr::Pallet::leaf_for_beefy_set`
// applies internally when it builds the `keyset_commitment` Merkle root.
// Doing it on the relayer side gives us the same set of addresses to
// build a validator tree from, with no need for the runtime to expose
// addresses directly.
func EthAddressFromBeefyPubkey(pk BeefyPubkey) ([20]byte, error) {
	parsed, err := ethcrypto.DecompressPubkey(pk[:])
	if err != nil {
		return [20]byte{}, fmt.Errorf("validators: decompress pubkey: %w", err)
	}
	var out [20]byte
	copy(out[:], ethcrypto.PubkeyToAddress(*parsed).Bytes())
	return out, nil
}

// EthAddressesFromBeefyPubkeys converts a whole validator set in one go.
// Returns the first error encountered, identifying which entry failed.
func EthAddressesFromBeefyPubkeys(keys []BeefyPubkey) ([][20]byte, error) {
	out := make([][20]byte, len(keys))
	for i, pk := range keys {
		addr, err := EthAddressFromBeefyPubkey(pk)
		if err != nil {
			return nil, fmt.Errorf("validators: key %d: %w", i, err)
		}
		out[i] = addr
	}
	return out, nil
}
