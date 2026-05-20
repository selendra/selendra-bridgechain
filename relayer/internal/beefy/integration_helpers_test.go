//go:build integration

package beefy

import (
	"encoding/hex"
	"encoding/json"
	"strings"
	"testing"
)

// unmarshalHex parses a JSON-encoded "0x..." string into the target.
func unmarshalHex(raw json.RawMessage, dst *string) error {
	return json.Unmarshal(raw, dst)
}

func mustHexDecode(t *testing.T, s string) []byte {
	t.Helper()
	s = strings.TrimPrefix(s, "0x")
	b, err := hex.DecodeString(s)
	if err != nil {
		t.Fatalf("hex decode: %v", err)
	}
	return b
}
