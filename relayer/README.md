# bridgechain — relayer (Go)

Off-chain daemon that bridges BEEFY commitments and outbound messages from
the bridgechain Substrate solochain to Ethereum, and (in time) carries
inbound events the other way.

This is a **skeleton**: it dials both ends, subscribes to BEEFY
justifications, decodes each one, and logs what would be submitted. The
commit-reveal flow for `BeefyClient.sol`, the MMR-leaf fetching, and the
beacon-chain inbound proofs are TODO — each gets its own follow-up.

## Layout

```
relayer/
├── cmd/relayer/main.go       — entry point, flags, signal handling
├── internal/
│   ├── substrate/client.go   — JSON-RPC over WebSocket
│   ├── beefy/relay.go        — subscribes + dispatches commitments
│   ├── beefy/commitment.go   — SCALE decoder for VersionedFinalityProof::V1
│   └── ethereum/client.go    — go-ethereum ethclient wrapper
└── go.mod
```

## Build & run

Requires Go 1.22+.

```bash
go build ./cmd/relayer
./relayer \
    --substrate-rpc ws://127.0.0.1:9944 \
    --ethereum-rpc ws://127.0.0.1:8545 \
    --gateway 0x... \
    --beefy-client 0x...
```

Each flag has a `BRIDGECHAIN_*` env-var fallback (see `cmd/relayer/main.go`).
Empty `--gateway` / `--beefy-client` puts the relayer in *skeleton mode*:
the destinations are logged, not actually called.

## Test

```bash
go test ./...
```

The SCALE decoder is exercised with hand-rolled bytes (block number,
validator set ID, payload, signatures) in `internal/beefy/commitment_test.go`.
Integration with a live node is not in the unit-test path.

## What still needs to be wired up

- Fetch MMR leaf + leaf proof via `mmr_generateProof` and the
  `BridgeOutboundApi::message_proof` runtime API.
- Drive `BeefyClient.sol` through `submitInitial → commitPrevRandao →
  submitFinal` (or `submitFiatShamir` if we end up using the
  non-interactive path).
- Carry a verified message through `Gateway.submitInbound` once we have a
  proof to give it.
- Inbound (Ethereum → Substrate): watch `Gateway.OutboundMessageAccepted`,
  build the receipt MPT proof, feed `bridge-inbound::submit`.

Useful upstream reference: Snowfork's `relayer/` Go module
(<https://github.com/Snowfork/snowbridge>) — same shape, parachain-specific
encoding. We share the commitment SCALE layout but not the MMR-leaf shape
(see `contracts/README.md` for our leaf encoding).
