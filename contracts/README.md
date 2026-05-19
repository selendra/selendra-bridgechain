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

## Leaf encoding

Both ends of the bridge agree on two keccak Merkle leaves:

- **BEEFY-MMR leaf** — SCALE encoding of `MmrLeaf { version: u8,
  parent_number_and_hash: (u32, H256), beefy_next_authority_set: (u64, u32,
  H256), leaf_extra: H256 }`. The Substrate side now types `leaf_extra` as
  `H256` so the wire format is a flat 32-byte tail with no compact-length
  prefix. `Gateway.hashMmrLeaf` mirrors this layout.

- **Per-message leaf** — `keccak256(SCALE(nonce ‖ destination ‖ payload))`.
  Substrate computes this via `Message::encode()` followed by
  `binary_merkle_tree::merkle_root::<Keccak256, _>`. The Solidity side
  reproduces it in `Gateway.hashMessageLeaf` using `ScaleCodec.encodeU64`,
  the destination bytes, and `ScaleCodec.checkedEncodeCompactU32` for the
  payload length. Cross-checked by
  `test_hashMessageLeafMatchesScaleEncoding`, including the compact-length
  boundary at 64.

## Next steps for this directory

1. Add a Foundry deployment script (`script/Deploy.s.sol`) once we know the
   initial validator-set merkle root and the target Ethereum network.
2. Hook up a test that exercises the full submitInbound → dispatch path
   with a real BEEFY commit-reveal flow. Snowfork's repo has a generator
   under `contracts/test/data/` we can adapt once the Substrate-side
   alignment lands.
3. Audit before any mainnet deploy. The contract surface is small; the
   commit-reveal logic inside `BeefyClient` is the consensus-critical bit.
