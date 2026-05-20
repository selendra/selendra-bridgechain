# Standalone Substrate Chain with BEEFY-backed Ethereum Bridge — Plan

**Status:** design doc / build plan
**Target:** a sovereign Substrate solochain (not a parachain) that bridges to Ethereum using BEEFY for cheap on-Ethereum verification of Substrate finality.

---

## 1. Goal

Build a standalone Substrate-based blockchain that:

- Runs its own consensus (BABE+GRANDPA or Aura+GRANDPA, plus **BEEFY** as a secondary finality gadget).
- Bridges to Ethereum **without going through Polkadot, AssetHub, or BridgeHub**.
- Verifies its finality on Ethereum cheaply by reusing the BEEFY scheme designed for the Polkadot↔Ethereum (Snowbridge) bridge.
- Verifies Ethereum finality on-chain using an Ethereum beacon-chain light client running inside the runtime.

This is a sovereign bridge: the chain owns the bridge end-to-end. There is no shared infrastructure.

---

## 2. Why BEEFY

GRANDPA is the primary finality gadget for Substrate chains. Its proofs are cheap to verify on other Substrate chains but expensive on EVM because they require many signature checks across a validator set whose membership Ethereum does not natively track.

**BEEFY** (Bridge Efficiency Enabling Finality Yielder) is a secondary gadget designed specifically for cross-chain consumers:

| Property | GRANDPA | BEEFY |
|---|---|---|
| Signature curve | ed25519 / sr25519 | **secp256k1** (native `ecrecover` on EVM) |
| Commits over | individual blocks | **MMR root** of all finalized blocks |
| Verification strategy on EVM | re-derive validator set per block, verify all sigs | **commit-reveal subsample** of signatures — gas independent of validator-set size |

A relayer submits one BEEFY commitment to Ethereum; the Ethereum side then trusts every Substrate block whose header sits under the committed MMR root.

---

## 3. How the bridge works end-to-end

### 3.1 Substrate → Ethereum direction (the cheap direction, enabled by BEEFY)

1. The runtime's outbound-message pallet receives messages destined for Ethereum and appends each one as a leaf into the chain's **MMR** (`pallet-mmr`).
2. At every finalized block, `pallet-beefy-mmr` produces a leaf containing the MMR root of all finalized headers up to that block plus the next BEEFY validator set hash.
3. BEEFY validators sign a `Commitment { payload: MMR_root, block_number, validator_set_id }` with their **secp256k1** keys. Signatures are gossiped via `sc-consensus-beefy` and aggregated into a `SignedCommitment`.
4. A **relayer** (off-chain) reads the latest `SignedCommitment` and submits it to the **BeefyClient** contract on Ethereum.
5. The BeefyClient verifies signatures using a two-phase **commit-reveal** scheme:
   - **Commit phase:** relayer submits the commitment + a bitfield of which validators signed.
   - **Reveal phase:** after a delay, the contract pseudo-randomly samples a small subset (e.g. log₂(N)) of validators using the previous block hash; relayer reveals signatures for that subset only.
   - This makes gas roughly **O(log N)** in validator-set size instead of O(N).
6. Once verified, the BeefyClient stores the MMR root.
7. To consume a specific message, the relayer submits:
   - the message,
   - a **leaf inclusion proof** against the MMR root, and
   - a **header proof** locating the message's block under that MMR leaf.
8. The **Gateway** contract validates the proofs against the stored MMR root and dispatches the message (token mint, contract call, etc.).

### 3.2 Ethereum → Substrate direction (no BEEFY — uses an in-runtime beacon light client)

1. The runtime hosts `snowbridge-pallet-ethereum-client`, an **Altair-fork Ethereum beacon-chain light client** that tracks the sync committee.
2. Off-chain relayers feed it beacon-chain updates (sync-committee rotations, finalized headers). The pallet verifies BLS sigs and stores execution-layer state roots.
3. `snowbridge-pallet-inbound-queue` accepts inbound messages: each carries a Merkle-Patricia proof that an event was emitted by the Gateway contract on Ethereum. The pallet verifies the proof against a stored execution state root, then dispatches.

### 3.3 ASCII data flow

```
            Substrate chain                                Ethereum
   ┌──────────────────────────────┐                ┌──────────────────────┐
   │ user → outbound-queue pallet │                │ user → Gateway.sol   │
   │              │ append leaf   │                │           │ emit evt │
   │              ▼               │                │           ▼          │
   │           pallet-mmr         │                │     event log        │
   │              │               │                │           │          │
   │              ▼               │                │           │          │
   │   pallet-beefy-mmr  ───┐     │                │           │          │
   │   (MMR root in leaf)   │     │                │           │          │
   │                        ▼     │                │           │          │
   │   pallet-beefy: signed       │  ── relayer ──▶│  BeefyClient.sol     │
   │   commitment over MMR root   │                │  (commit-reveal)     │
   │                              │                │           │          │
   │                              │                │           ▼          │
   │                              │  ◀── relayer ──│  Gateway.sol         │
   │   inbound-queue pallet  ◀────│                │  verifies & dispatch │
   │   verifies MPT proof against │                │                      │
   │   state root stored by       │                │                      │
   │   ethereum-client pallet     │                │                      │
   └──────────────────────────────┘                └──────────────────────┘
```

---

## 4. What this repo gives you for free

| Concern | Crate | Path |
|---|---|---|
| BEEFY runtime pallet | `pallet-beefy` | `substrate/frame/beefy/` |
| BEEFY MMR leaf provider | `pallet-beefy-mmr` | `substrate/frame/beefy-mmr/` |
| Generic MMR | `pallet-mmr` | `substrate/frame/merkle-mountain-range/` |
| BEEFY primitives (commitment, sig types) | `sp-consensus-beefy` | `substrate/primitives/consensus/beefy/` |
| BEEFY client gadget (gossip, signing, RPC) | `sc-consensus-beefy` | `substrate/client/consensus/beefy/` |
| Ethereum beacon-chain light client | `snowbridge-pallet-ethereum-client` | `bridges/snowbridge/pallets/ethereum-client/` |
| Inbound (Ethereum → Substrate) message queue | `snowbridge-pallet-inbound-queue` | `bridges/snowbridge/pallets/inbound-queue/` |
| Beacon-chain types, BLS verification | `snowbridge-beacon-primitives` | `bridges/snowbridge/primitives/beacon/` |
| Solochain template (starting point) | `solochain-template-*` | `templates/solochain/` |

---

## 5. What you must build yourself

1. **Outbound-commit pallet** — Snowbridge's `outbound-queue` is bonded to parachain semantics (it commits via parachain headers). For a solochain you write a simpler pallet that:
   - accepts outbound messages,
   - appends a digest of each into the MMR via `pallet-mmr`,
   - exposes a runtime API for relayers to fetch proofs.
2. **Ethereum-side Solidity contracts** — port from `snowbridge` (separate repo) but adapt to your validator-set encoding and message format:
   - `BeefyClient.sol` (commit-reveal verification, MMR root storage),
   - `Gateway.sol` (inbound dispatch + outbound emit),
   - your token / app contracts.
3. **Relayer** — a daemon that:
   - subscribes to BEEFY justifications via the node's RPC,
   - submits `SignedCommitment`s and MMR proofs to `BeefyClient`,
   - watches the Gateway and submits beacon updates + inbound messages to the Substrate side.
4. **Genesis & validator onboarding** — BEEFY keys (secp256k1) for each validator, key rotation flow.

---

## 6. Build plan

### Phase 1 — fork the template
- [ ] Copy `templates/solochain/` into a new workspace.
- [ ] Rename crates, update `Cargo.toml`, get `cargo check` clean.
- [ ] Confirm the node boots a single-validator dev chain.

### Phase 2 — add BEEFY to the runtime
- [ ] Add deps: `pallet-beefy`, `pallet-beefy-mmr`, `pallet-mmr`, `sp-consensus-beefy`.
- [ ] In `runtime/src/lib.rs`:
  - Add `BeefyId` to `SessionKeys`.
  - Implement `pallet_mmr::Config` (hashing = Keccak256 — required for cheap EVM verification, **not** the default BlakeTwo256).
  - Implement `pallet_beefy::Config` (key = `BeefyId`, max validators, equivocation reporting).
  - Implement `pallet_beefy_mmr::Config` (leaf data provider hooks).
  - Add all three to `construct_runtime!`.
- [ ] In `runtime/src/apis.rs` add:
  - `sp_consensus_beefy::BeefyApi`
  - `pallet_mmr::MmrApi`
  - `sp_consensus_beefy::mmr::BeefyMmrApi`

### Phase 3 — add BEEFY to the node
- [ ] Add dep: `sc-consensus-beefy`, `sc-consensus-beefy-rpc`.
- [ ] In `node/src/service.rs`: start the BEEFY worker after GRANDPA, wire its gossip protocol into the network config, plumb the justifications stream.
- [ ] In `node/src/rpc.rs`: register the BEEFY RPC (`beefy_subscribeJustifications`, `beefy_getFinalizedHead`).
- [ ] In `node/src/chain_spec.rs`: generate BEEFY (ecdsa) keys for genesis authorities, include in `SessionKeys`.
- [ ] Smoke test: run two validators, confirm signed commitments are produced and gossiped.

### Phase 4 — outbound side
- [ ] Write `pallet-bridge-outbound`: stores a per-message-nonce map, exposes `submit(destination, payload)`, on every block digests pending messages into a single commit and appends to MMR.
- [ ] Runtime API `outbound_messages_with_proof(block, nonce_range) -> Vec<(Message, MmrProof)>`.
- [ ] Benchmark and weight it.

### Phase 5 — inbound side
- [ ] Add deps: `snowbridge-pallet-ethereum-client`, `snowbridge-pallet-inbound-queue`, `snowbridge-beacon-primitives`.
- [ ] Configure `ethereum-client` for your target Ethereum network (Sepolia for testing, mainnet for prod) — genesis sync-committee, fork versions.
- [ ] Implement `MessageDispatch` for `inbound-queue` to route decoded messages into your application pallets.

### Phase 6 — Ethereum contracts
- [ ] Set up a Foundry/Hardhat project alongside the chain.
- [ ] Port `BeefyClient.sol` from snowbridge-ethereum; adapt validator-set commitment encoding to match what `pallet-beefy-mmr` emits in your runtime.
- [ ] Implement `Gateway.sol` with the message envelope your outbound pallet produces.
- [ ] Audit before mainnet — this is the most expensive single line in this plan.

### Phase 7 — relayer
- [ ] Fork the snowbridge relayer (Go); rewire it to your chain's RPC, your contract addresses, your message ABI.
- [ ] Run as a systemd service alongside an archive node.

### Phase 8 — integration
- [ ] Zombienet scenario: 4 validators + 1 relayer + Anvil (local Ethereum) + deployed contracts.
- [ ] End-to-end test: send a message Substrate→Ethereum, assert it lands; send one Ethereum→Substrate, assert it lands.
- [ ] Chaos test: drop relayer for N blocks, restart, ensure catch-up works.

### Phase 9 — production hardening
- [ ] Validator-set rotation tested across an era boundary.
- [ ] BEEFY equivocation slashing wired in.
- [ ] Beacon-client checkpoint update procedure documented.
- [ ] Operational runbook for stuck relayer, stale MMR root, beacon-fork events.

---

## 7. Risks and caveats

1. **You are shipping a custom light client on Ethereum.** The on-chain Solidity is consensus-critical and an attractive target. Budget an external audit.
2. **You run the bridge alone.** No shared relayer infrastructure — if your relayer goes down, the bridge stalls. Plan for >1 independent relayer operator.
3. **`pallet-mmr` hashing must be Keccak256.** The Substrate default (BlakeTwo256) cannot be verified cheaply on EVM. This is easy to get wrong and only shows up when you try to verify a proof on-chain.
4. **BEEFY validator-set rotation is the trickiest correctness boundary.** The Ethereum contract must accept a new set proven only by a signature from the old set. Test rotation explicitly.
5. **Beacon-chain hard forks** (e.g. Electra, post-Electra) require updating `snowbridge-pallet-ethereum-client`. Track the snowbridge upstream.
6. **Gas cost.** Even with commit-reveal, submitting commitments costs real ETH. Decide who pays — fee market on your chain or relayer subsidies.

---

## 8. Out of scope

- Token economics / fee design.
- Governance for upgrading the Ethereum contracts.
- Front-end / wallet integration.
- Multi-chain expansion (other EVM chains beyond Ethereum L1).

---

## 9. References

- BEEFY spec: `substrate/primitives/consensus/beefy/src/lib.rs`
- Solochain starting point: `templates/solochain/`
- Snowbridge inbound stack (reusable): `bridges/snowbridge/pallets/{ethereum-client,inbound-queue}/`
- Snowbridge outbound stack (reference only — parachain-coupled): `bridges/snowbridge/pallets/outbound-queue/`
