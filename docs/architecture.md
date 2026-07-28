# SelendraBridge Architecture

This document describes the system as implemented.
Every claim in it was checked against the source, and file and line references are given so you can check them again.
Where the code and this document disagree, the code is right and this document is a bug.

It replaced `BRIDGE_ARCHITECTURE.md`, which described a NestJS/TypeORM service that never existed in this repository and has been deleted.

---

## 1. What this is

An external-validator bridge, modeled on [deBridge's `DeBridgeGate`](https://github.com/debridge-finance/debridge-contracts-v1/tree/main/contracts/transfers).

The on-chain half is Solidity.
The off-chain half is Rust.
There is no TypeScript anywhere except the frontend.

The security model is a threshold multi-signature oracle.
A set of independent validators watch the source chain, each independently recomputes what it saw, and each signs only if its own computation matches the event.
A quorum of those signatures authorises a payout on the destination chain.
There is no fraud proof, no light client, and no optimistic window.
**If a threshold of validator keys is compromised, the bridge is compromised.**
Everything else in the design exists to make sure that is the *only* way to break it.

The asset model is lock and unlock, not mint and burn.
The destination gate holds pre-funded liquidity of a local ERC-20 registered against the incoming asset id.
A transfer moves value between two pools; it does not create tokens.

---

## 2. The one invariant that matters

Every transfer is identified by a `submissionId`, a keccak256 hash over its parameters.
The source contract computes it, and each validator independently recomputes it from the event log.
**A validator signs only when its own hash equals the one the contract emitted.**

That single equality check is what makes the validator an independent witness rather than a rubber stamp.
If the two implementations could ever disagree, the check would either block all traffic or, far worse, pass on parameters that differ from what the contract actually committed to.

So the hash is defined once in Solidity and reproduced in Rust, and the two are locked together by fixture-based tests that run on both sides:

- `contracts/src/BridgeHash.sol` is canonical.
- `contracts/test/GenFixtures.t.sol` generates `contracts/fixtures/submission_ids.json` from the Solidity.
- `crates/bridge-core/tests/equivalence.rs` and `crates/bridge-solana/tests/equivalence.rs` consume that file and assert byte equality.

This equivalence has been verified to hold: regenerating the fixtures from Solidity produces a byte-identical file, and the Rust suites pass against it.

Do not change `BridgeHash.sol` without regenerating the fixtures and running both test suites.

### 2.1 The preimages

All encoding is `abi.encodePacked`, chosen so alloy can reproduce it byte for byte off-chain.

**Asset id.**

```
debridgeId = keccak256(abi.encodePacked(nativeChainId, nativeToken))
```

A one-way hash of (origin chain, origin token address).
It travels in the message; the concrete local token address does not.
Each gate keeps its own `tokenOf[debridgeId] -> address` registry saying which local ERC-20 backs that asset on this chain.

Because keccak is not invertible, `Gate.Sent` also emits the concrete `token` address separately, since the refund relayer needs it to build a `refund()` call.

**Transfer id, without an execution payload.**

```
submissionId = keccak256(abi.encodePacked(
    SUBMISSION_PREFIX,   // uint256(1)
    debridgeId,
    chainIdFrom,
    chainIdTo,
    amount,
    receiver,            // dynamic bytes
    nonce
))
```

Note the field order: `chainIdFrom` and `chainIdTo` come *before* `amount`, which is not the order the arguments appear in any function signature.
Follow `BridgeHash.packedSubmission`, not intuition.

**Transfer id, with an execution payload.**
The seven-field packed base above, with five more fields appended before hashing:

```
submissionId = keccak256(abi.encodePacked(
    <the 7-field packed base>,
    autoParams.executionFee,
    autoParams.flags,
    keccak256(autoParams.fallbackAddress),
    keccak256(autoParams.data),
    keccak256(autoParams.nativeSender)
))
```

The three dynamic fields are hashed individually before being packed.
That is what keeps the concatenation unambiguous: without it, packed encoding of adjacent dynamic fields would admit collisions between different field splits.

**Refund-path digests.**

```
cancelId = keccak256(abi.encodePacked(CANCEL_PREFIX, submissionId))   // uint256(2)
refundId = keccak256(abi.encodePacked(REFUND_PREFIX, submissionId))   // uint256(3)
```

### 2.2 Why three prefixes

The prefixes are domain separators, and they are load-bearing.

A validator signature is just a signature over 32 bytes.
Without domain separation, the signature that authorises *paying out* a transfer would be a valid signature for *burning* it, and vice versa.
An attacker who collected a normal quorum of transfer signatures could replay them to cancel the transfer, then replay them again to refund it, and take the money twice.

Two things prevent that.
The prefix differs, and the preimage length differs: a cancel or refund preimage is 64 bytes, while a submission preimage is at least 224 bytes.
Crossing the domains would require a keccak256 preimage collision.

This is enforced on-chain and tested.
`contracts/test/Refund.t.sol` covers all four cross-domain replays by name:
`test_Cancel_RejectsReplayedTransferSignature`, `test_Cancel_RejectsReplayedRefundSignature`, `test_Refund_RejectsReplayedTransferSignature`, `test_Refund_RejectsReplayedCancelSignature`.
`crates/bridge-core/src/store.rs` tests the same property off-chain in `attestations_are_domain_separated`.

---

## 3. Contracts

All in `contracts/src/`, Solidity 0.8.24, built with `via_ir = true` and the optimizer on.

| Contract | Runtime size | Role |
| --- | --- | --- |
| `Gate.sol` | 5,983 B | The bridge. Deployed on every supported chain. |
| `SwapPool.sol` | 5,739 B | Same-chain swap, pegged pricing, reserve-capped. |
| `SwapRouter.sol` | 6,507 B | Composes swap and bridge into one cross-chain flow. |
| `BridgeHash.sol` | library | The canonical hashing. |
| `TestToken.sol` | test only | Mintable ERC-20 for local runs. |

### 3.1 Gate

One `Gate` per chain. It is both the source and the destination; the role depends on which function is called.

**State.**

| Mapping | Side | Meaning |
| --- | --- | --- |
| `nonceTo[chainIdTo]` | source | Monotonic per-corridor nonce. Makes each `submissionId` unique. |
| `sentBy[submissionId]` | source | Who locked the funds. Origin proof *and* authoritative refund recipient. Cleared on refund. |
| `refunded[submissionId]` | source | Refund replay guard. |
| `executed[submissionId]` | destination | Spent here. Set by **both** `claim` and `cancel`. |
| `cancelled[submissionId]` | destination | Distinguishes a burn from a delivery. |
| `tokenOf[debridgeId]` | destination | Asset registry: which local ERC-20 backs this asset id. |

The `executed` / `cancelled` split is a sharp edge worth internalising.
`executed` means "spent", not "delivered".
Any consumer that reads `executed` as proof of delivery must also check `cancelled`, or it will act on a payout that never happened.
`SwapRouter.finalize` gets this right at `SwapRouter.sol:227`, requiring `gate.executed(id) && !gate.cancelled(id)`.

**`send(token, amount, chainIdTo, receiver, autoParams)`** locks an ERC-20 and emits `Sent`.

The ordering inside `send` is deliberate and documented in the source.
The nonce is reserved, `sentBy` is written, and `Sent` is emitted **before** `safeTransferFrom` is called.
That is checks-effects-interactions: a token with a transfer hook that re-entered `send` would otherwise read the same nonce and emit a colliding `Sent`, desyncing the off-chain nonce sequence.
`Security.t.sol:test_Send_ReentrancyKeepsNoncesSequential` pins this.

**`claim(...)`** verifies a quorum, sets `executed`, then releases funds.
Effects before interactions again.

**`_verifySignatures(message, signatures)`** (`Gate.sol:524-542`) is the heart of the trust model:

```solidity
bytes32 digest = MessageHashUtils.toEthSignedMessageHash(message);
address last = address(0);
uint256 count = 0;
for (uint256 i = 0; i < signatures.length; i++) {
    address signer = ECDSA.recover(digest, signatures[i]);
    if (signer <= last) revert InvalidSignerOrder();   // strictly ascending
    if (isValidator[signer]) { count++; }
    last = signer;
}
if (count < threshold) revert NotEnoughSignatures(count, threshold);
```

Two things to note.
Signatures are EIP-191 (`eth_sign`) digests, not EIP-712.
And the **strictly ascending signer order is the deduplication mechanism**: it is what stops one validator's signature being submitted N times to fake a quorum.
Every caller must sort signatures by recovered signer address ascending.
`keeper::sorted_signatures` does this.

**Circuit breaker.**
`owner` or `guardian` may `pause`, halting `send` and `claim`.
Only `owner` may `unpause`.
The guardian is deliberately low-trust: it can stop the bridge but never start it and never move funds, so a compromised guardian causes a denial of service and nothing worse.

> **Operational note.** `setGuardian` is not called by any deploy path in this repository, so `guardian` is `address(0)` by default and only `owner` can trip the breaker. Set it as part of deployment.

### 3.2 SwapPool

A same-chain swap against a single stablecoin as the unit of account.
Not an AMM: prices are set by an oracle role, and each token's throughput is hard-capped by its own locked reserve.

`setPrice` enforces a per-update deviation cap (`maxPriceDeviationBps`) against the previous price.

> **Known gap.** The cap is per call, with no cooldown or time weighting. A compromised oracle key can walk the price arbitrarily far across repeated calls in a single block. See `report.md` M5.

`seedLiquidity` and `swap` both measure balance deltas across the transfer rather than trusting the requested amount, which is correct for fee-on-transfer tokens.

### 3.3 SwapRouter

Composes "swap on the source, bridge the stable, swap again on arrival" into one user flow.

`swapAndBridge` swaps the input token into the stable, then calls `gate.send`, encoding the destination intent (`finalToken`, `finalReceiver`, `finalMinOut`) into `autoParams.data`.
Because `autoParams` is folded into the `submissionId`, the destination intent is bound by the hash and cannot be altered in flight.

`finalize` is permissionless: anyone can complete the second leg for anyone else.
It requires `gate.executed(id) && !gate.cancelled(id)`, and is idempotent via its own `finalized[submissionId]` map.
If the destination swap cannot be satisfied, it falls back to delivering the stable rather than reverting.

---

## 4. Off-chain services

Eight crates. `crates/solana-gate` is excluded from the workspace because its `solana-program` dependencies do not build for the host target; it is built with `cargo build-sbf`.

| Crate | Kind | Responsibility |
| --- | --- | --- |
| `bridge-core` | lib | Canonical hashing, the signature store and **its trust boundary**, gate ABI bindings, allowlist. |
| `bridge-db` | lib | Postgres access via sqlx. Transaction history, refund lifecycle, allowlists. |
| `bridge-solana` | lib | Host-side reference model of the Solana gate, plus relayer log parsing. |
| `validator` | bin | Scan, recompute, sign, store. Also refund attestation. |
| `keeper` | bin | Collect a quorum, submit `claim` / `cancel` / `refund`. |
| `sig-store` | bin | HTTP signature store (axum). The shared bulletin board. |
| `indexer` | bin | Read-only chain observer. **Sole writer of `refund_status`.** |
| `graphql-api` | bin | Read API for the frontend. |

### 4.1 Which processes are required for which features

This is not obvious from the code, and getting it wrong silently disables features rather than failing loudly.

| Feature | Requires |
| --- | --- |
| Basic transfer (send, claim) | `validator`, `keeper`, `sig-store` |
| Transaction history, stuck detection | plus `indexer`, Postgres |
| **Refunds** | plus `indexer` (it is the only writer of `refund_status`) |
| Frontend | plus `graphql-api` |

> **Operational note.** The `Dockerfile` builds only `validator`, `keeper`, and `sig-store`, and `docker-compose.yml` deploys only those. In that stack the refund lifecycle never advances and the frontend has no backend. See `report.md` H2.

### 4.2 validator

One independent scan loop per configured source chain.

Per batch: fetch logs up to `latest - block_confirmation`, and for each `Sent`:

1. Decode the event.
2. **Independently recompute the `submissionId`** from the decoded fields.
3. Compare against the emitted id. On mismatch, refuse and log loudly.
4. Check the nonce is sequential for that corridor. A gap or a replay pauses the scanner rather than guessing.
5. Check the token and corridor against the allowlist.
6. Sign the EIP-191 digest and write to the store.

Cursor and per-corridor nonce state persist to `state_file` so a restart resumes rather than replays.

An operator HTTP API (`validator/src/api.rs`) exposes `/status`, and `/pause`, `/resume`, `/rescan`, each optionally per chain.

> **Known gaps.** A batch whose `handle_log` returns `Err` still advances the cursor, so that transfer is never signed and never retried. The `paused` flag is runtime-only and a restart clears it. `block_confirmation` defaults to 0 with no validation, and the `allow_zero_confirmation` key present in all three shipped configs is not a field on the config struct, so serde discards it silently. See `report.md` M2 and M4.

### 4.3 validator refund attestation

A separate loop (`validator/src/refund.rs`) with its own safety rules, and the most carefully written code in the repository.

It polls the store for stuck candidates and decides **from on-chain facts alone**, never from what the database claims:

- It reads both chains at a **confirmed block, not the tip**. Reading `executed` at the tip would let a reorg make a claimed transfer look unclaimed, and the loop would then attest a cancel for a transfer that was actually paid.
- It refuses to vote on any corridor where it cannot read *both* ends. Attesting on a chain it cannot read would mean trusting the store's word for whether a transfer was delivered, which is precisely what an attacker would want.
- **A claimed transfer never earns a cancel or refund attestation, whatever the store says about timeouts.**
- A refund is never attested for a `submissionId` this gate never emitted.

The decision function is split out from the I/O specifically so these rules are unit-testable, and five tests cover them.

### 4.4 keeper

Polls the store and submits transactions once a quorum exists.

Three loops: claim on the destination, cancel on the destination, refund on the source.
Cancels are checked **before** the transfer-threshold and allowlist gates, deliberately: those gates protect payouts, and a cancel is the opposite of a payout.
Checking them first would strand exactly the transfers that most need refunding, since an allowlist-rejected transfer never collects transfer signatures at all.

The cancel and refund paths deliberately discard their own transaction hashes, letting the indexer record state from the observed on-chain event.
The comment in the source states the principle: the keeper's word is not authoritative for a state that gates the refund-candidate list.

> **Known gap.** The claim path violates that principle. `try_claim` does not check `receipt.status()`, so a reverted claim is recorded as a success via `mark_claimed`, which permanently excludes the transfer from the refund sweep. See `report.md` H1.

### 4.5 sig-store

An axum HTTP service, the shared bulletin board validators write to and keepers read from.
Postgres-backed, via `bridge-db`.

A validator does not have to use it.
`validator::sink::Sink` picks its backend from config: `[store] url = ...` selects the HTTP sig-store, `[store] dir = ...` selects a local filesystem store, and it refuses to start if neither is set.
Single-validator local runs use the file path; a real deployment uses the HTTP path so multiple validators share one view.

**The same guards apply on both paths.**
`bridge-db` does not reimplement validation; it calls the exact functions from `bridge_core::store` (`canonical_submission_id`, `verify_signature`, `verify_token_binding`, `same_params`, `verify_attestation`, `is_valid_submission_id`) at `bridge-db/src/lib.rs:272-309` and `:447-460`.
This is the right structure: there is one definition of what a valid record is, and swapping the storage backend cannot weaken it.

Every route except `/health` requires a bearer token (`SIG_STORE_TOKEN`), which is tested four ways.

Routes: `/submissions` (post, list), `/submissions/:id`, `/submissions/:id/claimed`, `/submissions/:id/attestations`, `/refund-candidates`, `/history`, and allowlist management under `/allowed/`.

**The sig-store is untrusted infrastructure.**
It is a convenience for distribution, not a source of authority.
Its operator cannot forge a transfer, because the on-chain `_verifySignatures` counts only real validator signatures.
The guards in the next section are what make that true.

### 4.6 indexer

Read-only. Never signs, never sends a transaction.

Exists so every transfer is visible in the database, **including those with zero validator signatures**, which are invisible to the signature-store view by construction.

Observes `Gate.Sent`, `Gate.Claimed`, `SwapPool.Swapped`, `SwapRouter.SwapBridged`, `SwapRouter.Finalized` / `FinalizeFallback`, and runs the refund-eligibility sweep.

> **Known gap.** A failed scan still advances the cursor, permanently dropping every event in that block range. See `report.md` M2.

---

## 5. The trust boundary

`crates/bridge-core/src/store.rs` is the single most security-critical file in the off-chain system.
Everything arriving at the store is treated as hostile, including input from other validators.

Read it before changing anything near it.

These guards are defined once and applied on **every** storage path.
The filesystem store calls them directly; `bridge-db` calls the same functions rather than reimplementing them.
Keep it that way.

| Guard | Function | Prevents |
| --- | --- | --- |
| Id and parameter binding | `canonical_submission_id` | A record whose claimed id does not hash from its own parameters. |
| Parameter immutability | `same_params` | Poisoning an existing record's fields on a later write. |
| Signature recovery | `verify_signature` | A signature that does not recover to its claimed signer. |
| Token binding | `verify_token_binding` | Substituting a more valuable asset. `token` is not covered by the `submissionId`, so it is recomputed against `debridgeId`. |
| Domain separation | `verify_attestation` | Replaying a transfer signature as a cancel or refund. |
| Path traversal | `is_valid_submission_id` | A crafted id escaping the store directory. |

Nine tests, each named after the attack it blocks:
`rejects_id_param_mismatch`, `rejects_param_poisoning_of_existing_record`, `rejects_forged_signature`, `rejects_token_not_matching_debridge_id`, `attestations_are_domain_separated`, `attestation_requires_an_existing_record`, `rejects_garbage_signature`, `happy_path_two_validators_merge_and_dedupe`.

> **Known gap.** `verify_signature` confirms the signature recovers to the claimed signer, but does **not** check that the signer is in the validator set. Anyone who can reach the store can inflate `signature_count` with well-formed signatures from arbitrary keys. This is not exploitable for fund loss, because the on-chain check counts only `isValidator[signer]`, but it makes the off-chain view of "how many validators signed" attacker-controlled. See `report.md` M3.

---

## 6. Transfer lifecycle

### 6.1 Happy path

```
SOURCE CHAIN                          OFF-CHAIN                      DESTINATION CHAIN

user: approve(gate, amount)
user: gate.send(...)
  nonce reserved
  sentBy[id] = msg.sender
  emit Sent(id, ...)      ─────────►  validator (xN)
  safeTransferFrom                      recompute id
                                        id == emitted? ──── no ──► refuse, log
                                        yes: check nonce sequence
                                             check allowlist
                                             sign EIP-191 digest
                                             POST to sig-store
                                                  │
                                                  ▼
                                              keeper polls
                                              sigs >= threshold?
                                              sort ascending
                                                  │
                                                  └──────────►  gate.claim(...)
                                                                  _verifySignatures
                                                                  executed[id] = true
                                                                  safeTransfer(to, amount)
                                                                  emit Claimed
                                                                       │
                                                indexer ◄──────────────┘
                                                  mark_claimed
```

### 6.2 Refund path

A transfer can strand: the destination gate may hold no liquidity for the asset, the corridor may be de-listed after funds were locked, or the destination chain may be down long enough that nobody claims.

The locked funds must be returnable.
But **a refund that merely waits out a timeout is a double-spend**: the transfer's validator signatures still exist, so a keeper can `claim()` on the destination in the same window the source pays the refund, releasing the same value twice.

The fix is to order the two legs and enforce that ordering **on-chain, not by any timing assumption**:

1. **`cancel()` on the destination** burns `executed[submissionId]`.
   From that moment `claim()` reverts with `AlreadyExecuted`.
   The destination can never pay out, permanently and verifiably.
   Moves no funds.
2. **Validators observe the resulting `Cancelled` event** at a confirmed block and only then sign the refund digest.
   This is an ordinary on-chain fact, attested exactly like a `Sent`.
3. **`refund()` on the source** returns the funds to `sentBy[submissionId]`.

If a keeper wins the race and claims first, step 1 reverts and no refund is ever authorised.
There is no interleaving that pays out twice.

`refund()` stands behind three independent guards:

- `sentBy[submissionId]` must be non-zero. This is the only proof this gate really sent this id, and it is also the payout address. For a plain transfer `nativeSender` is not folded into the hash, so a caller could name any address in calldata; **storage is authoritative, calldata is not trusted**.
- A validator threshold over `getRefundId(...)`, whose quorum only forms after `Cancelled` is observed.
- `keccak256(block.chainid, token)` must equal `debridgeId`, so a caller cannot name a different, more valuable asset held by the gate.

Twenty-three tests in `contracts/test/Refund.t.sol` cover this path.

---

## 7. Solana

Two separate things, and they have diverged.

`crates/bridge-solana` is a **host-side reference model** used for hash-equivalence tests and relayer log parsing. It models an asset registry and vault liquidity.

`crates/solana-gate` is the **deployable BPF program**. It is excluded from the workspace and absent from compose.

> **Do not deploy `solana-gate` as written.** It has no PDA or owner validation on the config account outside `process_init`, no asset registry, and no liquidity checks. Its `emit_sent` writes 64 bytes and discards the rest, so the emitted event does not contain what a validator needs, and its format is incompatible with the relayer's own parser. The reference model is the more correct of the two. See `report.md` L7 and L8.

Treat EVM to Solana as unfinished.

---

## 8. Configuration and secrets

Validators and keepers are configured by TOML (`validator/src/config.rs`, `keeper/src/config.rs`).
Examples live in `docker/configs/`.

Two things to know before running this anywhere real.

**`block_confirmation` is your reorg protection.**
It defaults to `0`, is not validated, and the config struct does not use `deny_unknown_fields`, so a typo'd safety key is silently ignored.
Set it explicitly per chain.

**Private keys are currently inline in the config TOMLs.**
The files in `docker/configs/` contain anvil's well-known development keys, which is fine for local runs.
`.dockerignore` now excludes them from the build context, but the pattern is still wrong: `SignerConfig` supports an encrypted keystore and an env var, and a real key should use one of those rather than the file.
See `docs/operations.md` for the key-custody options.

The sig-store bearer token comes from `SIG_STORE_TOKEN` and defaults to `dev-local-bridge-token` in compose.
Override it.

---

## 9. Testing

| Suite | Command | Count |
| --- | --- | --- |
| Solidity | `forge test` (from `contracts/`) | 99 |
| Rust | `cargo test --workspace` | 40 |
| End-to-end | `scripts/testing/*.sh` | 29 scripts |

Solidity breakdown: `Swap` 27, `Refund` 23, `Security` 20, `SwapRouter` 9, `Claim` 8, `SolanaBridge` 6, `Send` 5, `GenFixtures` 1.

Requires Foundry, plus `forge install foundry-rs/forge-std@v1.9.4 OpenZeppelin/openzeppelin-contracts@v5.0.2` (`contracts/lib` is gitignored and vendored, not committed).

If you touch `BridgeHash.sol`, run `forge test` **and** `cargo test --workspace`.
The first regenerates the fixtures, the second checks Rust still agrees.

> **Known gaps.** `forge coverage` does not run on this project (stack-too-deep at `Gate.sol:308`), so coverage has never been measured. The `scripts/testing/*.sh` suite is currently broken: the scripts resolve their root as `dirname/..`, but they live in `scripts/testing/`, so the path lands one directory short. See `report.md` L12 and section 8.2.

---

## 10. Where to look first

Reading in this order will get you oriented fastest.

1. `contracts/src/BridgeHash.sol` (110 lines). The whole system is built on this.
2. `contracts/src/Gate.sol`, specifically `send`, `claim`, and `_verifySignatures`.
3. `crates/bridge-core/src/store.rs`. The trust boundary, and its nine attack-named tests.
4. `crates/validator/src/main.rs`, the `handle_log` recompute-and-compare step.
5. `crates/validator/src/refund.rs`, the `decide` function. The clearest statement of the system's safety rules.
6. `contracts/test/Refund.t.sol`. Twenty-three tests that explain the double-spend problem better than prose can.

For the current list of known defects and their priority, see `report.md` in the repository root.
