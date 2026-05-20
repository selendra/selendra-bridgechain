# bridgechain — build status & handoff

Snapshot of where the bridge implementation stands, what works end-to-end,
and the concrete next steps. Tracks against
[`BEEFY-Ethereum-Solochain-Plan.md`](./BEEFY-Ethereum-Solochain-Plan.md).

Last updated: 2026-05-20.

---

## TL;DR

| Component | State |
|---|---|
| Substrate node (BEEFY + MMR + GRANDPA) | ✅ runs, signs commitments |
| `pallet-bridge-outbound` | ✅ stores messages, exposes keccak Merkle root via BEEFY-MMR `leaf_extra` |
| `pallet-bridge-inbound` (stub verifier) | ✅ accepts proofs, replay-checks, dispatches via trait |
| `contracts/BeefyClient.sol` (Snowfork port) | ✅ compiles, deployment smoke tests pass |
| `contracts/Gateway.sol` (custom) | ✅ compiles, SCALE-leaf encoding cross-checked |
| Go relayer (`relayer/`) | 🟡 decodes commitments and fetches submission bundles, **doesn't submit yet** |
| Real Ethereum beacon-client verifier | ❌ parked in `vendor/snowbridge/`, not built |
| End-to-end Substrate → Ethereum integration | ❌ relayer can't generate MMR-leaf proofs yet |
| End-to-end Ethereum → Substrate integration | ❌ inbound is on `MockVerifier` |
| Production hardening (rotation, slashing) | ❌ |

---

## What's done

### Substrate side (Rust)

**BEEFY stack** in `runtime/`:
- `pallet-session`, `pallet-beefy`, `pallet-mmr` (Keccak256, **not** BlakeTwo256
  — required for cheap EVM verification), `pallet-beefy-mmr`.
- `SessionKeys { aura, grandpa, beefy }`. Node service starts the BEEFY worker
  alongside GRANDPA; gossip protocol is wired up and the justifications
  stream is exposed via RPC.

**Outbound pallet** in `pallets/bridge-outbound/`:
- `submit(destination: H160, payload: BoundedVec<u8>)` — append-only per-block
  message log, monotonic nonce.
- Per-block keccak Merkle root over `SCALE(Message)` leaves, pinned into
  the BEEFY-MMR leaf's `leaf_extra` (typed as `H256`, flat 32 bytes — see
  the leaf-encoding fix below).
- Runtime API `BridgeOutboundApi { messages_at, commitment_root_at,
  message_proof }`.

**Inbound pallet** in `pallets/bridge-inbound/`:
- Pluggable `Verifier` trait. MVP `MockVerifier` accepts every proof — **swap
  before any production deploy.**
- `submit(message_origin, nonce, payload, log, proof)` verifies → checks
  gateway origin → checks nonce not used → dispatches via
  `MessageDispatch` trait → emits `MessageReceived`.
- Runtime API `BridgeInboundApi { latest_nonce, is_nonce_used }`.

**Leaf-encoding alignment** (commit `f1d0fa9`):
- `pallet_beefy_mmr::Config::LeafExtra = sp_core::H256`.
- `impl BeefyDataProvider<H256> for Pallet<T>`; empty blocks emit
  `H256::zero()` so the leaf is always a flat 32 bytes.
- The Solidity `Gateway.hashMmrLeaf` mirrors this layout — proofs round-trip.

### Ethereum side (Solidity)

`contracts/` is a Foundry project (`solc 0.8.34`).

**Ported from Snowfork** (Apache-2.0, attribution preserved):
- `src/BeefyClient.sol` — interactive commit-reveal + Fiat-Shamir
  verification of BEEFY commitments.
- `src/utils/{Bitfield, Bits, Math, MMRProof, ScaleCodec, Uint16Array,
  SubstrateMerkleProof}.sol`.

**Custom**:
- `src/Gateway.sol` (~180 lines):
  - `sendMessage(bytes)` for Ethereum→Substrate (just emits an event for
    the relayer to pick up).
  - `submitInbound(message, leaf, leafProof, msgProof)` for
    Substrate→Ethereum:
    1. Hash the BEEFY-MMR leaf using `hashMmrLeaf` (SCALE-mirroring).
    2. Verify against `BeefyClient.latestMMRRoot` via
       `verifyMMRLeafProof`.
    3. Hash the message using `hashMessageLeaf`
       (`keccak256(SCALE(nonce ‖ destination ‖ payload))`).
    4. Verify against `leaf.leafExtra` via
       `SubstrateMerkleProof.verify`.
    5. Replay-check by nonce, then call into the destination.
- `src/interfaces/IGateway.sol`.

**Tests** (`forge test`): 7 passing.
- Deployment + constructor validity.
- `sendMessage` nonce/event emit.
- `hashMessageLeaf` matches hand-rolled SCALE encoding.
- `hashMessageLeaf` compact-length boundary at 64.
- `hashMmrLeaf` matches hand-rolled SCALE encoding.
- `submitInbound` reverts with `InvalidMmrLeafProof` when the MMR root is
  unknown.

### Relayer (Go)

`relayer/` Go module (Go 1.24). **Skeleton — submission paths are stubbed.**

- `cmd/relayer/main.go` — flags + env vars, signal handling, two long-running
  loops (only the Substrate→Ethereum side is wired).
- `internal/substrate/client.go` — JSON-RPC over WebSocket. Hand-rolled
  (no `go-substrate-rpc-client` dependency). Supports `Call`, `Subscribe`,
  per-sub channel fan-in, graceful shutdown.
- `internal/beefy/relay.go` — subscribes to `beefy_subscribeJustifications`
  and logs each decoded commitment.
- `internal/beefy/commitment.go` — SCALE decoder for
  `VersionedFinalityProof::V1` (commitment payload, block number, validator
  set id, `Vec<Option<[u8;65]>>` signatures). Unit-tested with hand-rolled
  bytes.
- `internal/ethereum/client.go` — `ethclient` wrapper, captures chain ID.

In skeleton mode (empty `--gateway` / `--beefy-client`), the relayer logs
what would be submitted instead of sending. Useful for wiring a dev node
without on-chain side-effects.

### Parked work

`vendor/snowbridge/` — primitives + ethereum-client pallet copied from
`polkadot-stable2509-4-rc1`, **excluded from the workspace**. Finishing the
vendoring would let us swap `MockVerifier` for the real
`snowbridge-pallet-ethereum-client`. Blocker is dependency grinding (~15
crates with their own workspace conventions) plus stripping XCM from
`snowbridge-core`. See `vendor/snowbridge/README.md` for the punch list.

---

## What to do next

Suggested order — each item builds on the previous.

### 1. Build the MMR-proof fetcher in the relayer

**Mostly done** (see commits after `64dafc6`). Three new Go packages:

- `internal/scale` — shared SCALE reader (compact, LE primitives, byte
  slices, fixed 32-byte reads). Unit-tested across all compact-int modes.
- `internal/mmr` — `Leaf` (bridgechain shape with H256 leaf_extra),
  `Proof`, decoders for `mmr_generateProof`'s SCALE-encoded leaves and
  proof fields, and `Leaf.LeafHash()` that produces the keccak hash the
  Solidity Gateway expects (cross-checked against Foundry test fixture).
- `internal/outbound` — `FetchMessageBundle(block, index, ...)` that calls
  `BridgeOutboundApi::message_proof` and `messages_at` via `state_call`,
  plus `mmr_generateProof` for the leaf inclusion. Returns a `Bundle` with
  everything `Gateway.submitInbound` wants — the message, the per-block
  Merkle proof, the BEEFY-MMR leaf, and the MMR proof.

**Still open — proof-order computation.** `MMRProof.verifyLeafProof` in
BeefyClient.sol takes a `proofOrder` bitfield. mmr-lib (Rust) derives it
from `leaf_index + leaf_count` via `gen_proof_positions`. Porting that to
Go is straightforward but easy to subtly mis-implement — wrong order bits
produce a wrong root, which the contract rejects as
`InvalidMmrLeafProof`. Plan: add a runtime-API helper on the Substrate
side (`BridgeOutboundApi::mmr_proof_order(leaf_index, leaf_count)`) so
both ends share the same Rust implementation. Tracked in
`relayer/internal/mmr/types.go`.

### 2. Wire up the commit-reveal driver

The BeefyClient flow is:

1. `submitInitial(commitment, bitfield, oneSignerProof)` — open a ticket.
2. Wait `randaoCommitDelay` Ethereum blocks.
3. `commitPrevRandao(ticketID)` — capture the randomness for sampling.
4. `createFinalBitfield(ticketID, bitfield)` — get the validator subset.
5. `submitFinal(ticketID, commitment, bitfield, validatorProofs[],
   leaf, leafProof, leafProofOrder)` — close the ticket, publish the new
   MMR root.

Each step needs:

- An ECDSA-signed transaction (use go-ethereum's `bind.NewKeyedTransactor`).
- The validator set merkle tree from the runtime API
  `pallet_beefy_mmr::Pallet::authority_set_root`, so we can build the
  inclusion proofs `BeefyClient.isValidatorInSet` expects.
- The full `ValidatorProof` structs (v/r/s + leaf index + leaf hash +
  inclusion proof) for each sampled validator.

Bind this to the contracts via `abigen`:

```bash
forge build --extra-output-files abi
abigen --abi out/BeefyClient.sol/BeefyClient.json --pkg bindings \
       --out relayer/internal/bindings/beefy_client.go --type BeefyClient
abigen --abi out/Gateway.sol/Gateway.json --pkg bindings \
       --out relayer/internal/bindings/gateway.go --type Gateway
```

### 3. End-to-end smoke test

With (1) and (2) done:

1. Start a dev bridgechain node (`./node --dev --alice --tmp`).
2. Start `anvil` (`anvil --port 8545`).
3. Deploy `BeefyClient.sol` and `Gateway.sol` against Anvil, using the
   dev node's genesis BEEFY authority set as constructor input.
4. Start the relayer.
5. Call `pallet-bridge-outbound::submit(...)` on the Substrate side.
6. Watch the relayer drive commit-reveal, then submit the message via
   `Gateway.submitInbound`.
7. Assert the destination address gets the call.

Codify as a Go integration test (build-tag `integration`) or a shell
script under `scripts/`.

### 4. Inbound (Ethereum → Substrate)

Currently the inbound pallet uses `MockVerifier`. Path forward:

**Path A — keep `MockVerifier` for the demo.** Easy. Lets us showcase
two-way flow before any beacon-chain plumbing. Production-unsafe.

**Path B — finish vendoring `snowbridge-pallet-ethereum-client`.** This is
the parked work in `vendor/snowbridge/`. Replaces `MockVerifier` with a
real Altair beacon-chain light client. Requires:

1. Adding ~15 transitive deps to `Cargo.toml` workspace (alloy-*,
   ssz_rs, milagro-bls, ethabi-decode, sp-crypto-hashing, …).
2. Stripping XCM coupling from `snowbridge-core` (some already done).
3. Re-including `vendor/snowbridge/{primitives,pallets}` in the workspace
   `members` list.
4. Genesis: a starting sync committee + finalized header for the target
   network (Sepolia for testing, mainnet for prod).
5. Updating `runtime/src/configs/mod.rs` to set
   `type Verifier = snowbridge_pallet_ethereum_client::Pallet<Runtime>`.

### 5. Production hardening (plan-doc Phase 9)

- Validator-set rotation tested across an era boundary (currently sessions
  are static — `PeriodicSessions` with a 1-year period).
- BEEFY equivocation slashing wired in (currently `EquivocationReportSystem
  = ()`).
- Audit `Gateway.sol`. The Snowfork BeefyClient has been audited upstream;
  our additions have not.
- Operational runbook: stuck relayer, stale MMR root, beacon-fork events.
- Plan for >1 independent relayer operator (single-relayer = bridge stalls
  if it goes down).

---

## How to run things

All commands run inside WSL Ubuntu (`wsl -d Ubuntu`) — not Git Bash.

```bash
# Substrate
cd ~/project/bridgechain
SKIP_WASM_BUILD=1 cargo check --workspace
cargo test --workspace
cargo build --release -p solochain-template-node

# Solidity
cd ~/project/bridgechain/contracts
# one-time after clone:
forge install --no-git foundry-rs/forge-std
forge install --no-git OpenZeppelin/openzeppelin-contracts@v5.0.2
forge build
forge test

# Relayer
export PATH="$HOME/.local/go/bin:$PATH"
cd ~/project/bridgechain/relayer
go test ./...
go build -o /tmp/relayer ./cmd/relayer
/tmp/relayer --substrate-rpc ws://127.0.0.1:9944 --ethereum-rpc ws://127.0.0.1:8545
```

---

## File map

```
bridgechain/
├── node/                                — Substrate node binary (BEEFY wired)
├── runtime/                             — Runtime (BEEFY + MMR + bridge pallets)
├── pallets/
│   ├── bridge-outbound/                 — Substrate → Ethereum messages
│   ├── bridge-inbound/                  — Ethereum → Substrate messages
│   └── template/                        — leftover template, can prune
├── contracts/                           — Foundry / Solidity
│   ├── src/BeefyClient.sol              — ported, Snowfork
│   ├── src/Gateway.sol                  — custom MVP
│   ├── src/interfaces/IGateway.sol
│   ├── src/utils/*.sol                  — ported, Snowfork
│   └── test/*.t.sol                     — 7 passing tests
├── relayer/                             — Go skeleton
│   ├── cmd/relayer/                     — entry point
│   └── internal/{substrate,beefy,ethereum}/
├── vendor/snowbridge/                   — parked, excluded from workspace
└── docs/
    ├── BEEFY-Ethereum-Solochain-Plan.md — design doc / build plan
    ├── STATUS.md                        — this file
    └── rust-setup.md
```

---

## Recent commits

```
64dafc6 feat(relayer): Go relayer skeleton with BEEFY commitment decoder
f1d0fa9 fix(bridge): align BEEFY-MMR leaf_extra type to H256 across both sides
6e9ea7e first commit  (BEEFY+MMR runtime, bridge pallets, Foundry contracts)
```
