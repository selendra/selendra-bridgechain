# Vendored Snowbridge (parked)

Source: [polkadot-sdk @ `polkadot-stable2509-4-rc1`](https://github.com/paritytech/polkadot-sdk)

Extracted with `git archive` from the upstream tag — see `/tmp/vendor.sh` (used
during the initial vendoring).

## Why these crates live here

The plan calls for `snowbridge-pallet-ethereum-client` to verify Ethereum
finality on-chain. None of the snowbridge crates are published to crates.io, so
they have to live in-tree.

## Why they are not yet built

These manifests are **excluded from the workspace** in `bridgechain/Cargo.toml`.
They reference upstream workspace deps (`alloy-*`, `ssz_rs`, `milagro-bls`,
`ethabi-decode`, `sp-crypto-hashing`, …) and lint config that the bridgechain
workspace hasn't fully wired in yet. Pulling them in cleanly requires:

- adding ~15 transitive workspace deps with the right crates.io versions,
- surgery on `snowbridge-core` to drop XCM coupling (started: `src/lib.rs` and
  unused source files already removed),
- removing upstream `[lints]` blocks (done),
- mapping `use Debug;` style upstream conventions to local imports,
- writing a Config impl for the `EthereumClient` pallet that targets a real
  Ethereum network (Sepolia / mainnet) including the genesis sync committee.

## What lives in-tree instead

`pallets/bridge-inbound/` defines a `Verifier` trait that the eventual real
`EthereumClient` will implement. For now the stub `MockVerifier` accepts any
proof; the inbound flow (replay protection, dispatch) is otherwise complete.
When the vendoring is finished, swap the `type Verifier` line in the runtime
Config.
