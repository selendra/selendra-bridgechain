# bridgechain — Ethereum-side contracts

Solidity counterpart of the bridgechain Substrate solochain.

## Layout

```
src/
├── BeefyClient.sol            # Snowfork BEEFY light client (Apache-2.0)
├── Gateway.sol                # Bridgechain-specific bridge endpoint
├── interfaces/IGateway.sol    # Public types + events
└── utils/                     # Snowfork helpers used by BeefyClient
    ├── Bitfield.sol           ├── MMRProof.sol
    ├── Bits.sol               ├── ScaleCodec.sol
    ├── Math.sol               ├── SubstrateMerkleProof.sol
    └── Uint16Array.sol

test/
├── BeefyClient.t.sol          # Deployment smoke tests
└── Gateway.t.sol              # sendMessage + leaf-hash + revert paths
```

## Build & test

After cloning, install the Solidity dependencies (`lib/` is gitignored):

```bash
forge install --no-git foundry-rs/forge-std
forge install --no-git OpenZeppelin/openzeppelin-contracts@v5.0.2
```

Then:

```bash
forge build
forge test
```

## Provenance

`BeefyClient.sol` and everything in `src/utils/` are taken verbatim from
[Snowfork/snowbridge](https://github.com/Snowfork/snowbridge) (Apache-2.0).
The original SPDX-FileCopyrightText headers are preserved. Snowbridge owns the
correctness of those files; this project owns `Gateway.sol`,
`interfaces/IGateway.sol`, and the tests.

## Outstanding alignment work

The Substrate side currently types the BEEFY-MMR `leaf_extra` as `Vec<u8>`,
which SCALE-encodes with a compact-length prefix. `Gateway.hashMmrLeaf`
assumes a flat `bytes32`. Pick one before integration:

- **Substrate-side fix (recommended):** change
  `BeefyDataProvider<Vec<u8>>` to `BeefyDataProvider<H256>` in
  `bridgechain/pallets/bridge-outbound/src/lib.rs`, set
  `pallet_beefy_mmr::Config::LeafExtra = H256` in the runtime config, and
  return `H256::zero()` (not `Vec::new()`) for empty blocks.

- **Solidity-side fix:** extend `Gateway.hashMmrLeaf` to compact-encode the
  leaf_extra length before the bytes. Slightly more code here and any other
  consumer of the MMR-leaf shape would have to mirror it.

The per-message leaf encoding `keccak256(SCALE(nonce ‖ destination ‖
payload))` is exercised end-to-end by
`test_hashMessageLeafMatchesScaleEncoding` and matches what
`pallet-bridge-outbound::commitment_root_for` produces. No alignment work
needed there.

## Next steps for this directory

1. Add a Foundry deployment script (`script/Deploy.s.sol`) once we know the
   initial validator-set merkle root and the target Ethereum network.
2. Hook up a test that exercises the full submitInbound → dispatch path
   with a real BEEFY commit-reveal flow. Snowfork's repo has a generator
   under `contracts/test/data/` we can adapt once the Substrate-side
   alignment lands.
3. Audit before any mainnet deploy. The contract surface is small; the
   commit-reveal logic inside `BeefyClient` is the consensus-critical bit.
