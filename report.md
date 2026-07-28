# SelendraBridge Code Review

Date: 2026-07-28
Reviewer: Claude Opus 5
Method: read the code as the source of truth, documentation treated as untrusted.

---

## 1. Scope and method

Everything reviewed lives under `./bridge/`.
The repository root holds only `README.md` (a one-line stub), `docs/`, and `.gitignore`.

Reviewed in full:

| Area | Files |
| --- | --- |
| Contracts | `Gate.sol`, `BridgeHash.sol`, `SwapPool.sol`, `SwapRouter.sol`, `TestToken.sol` |
| Rust services | `bridge-core`, `bridge-db`, `bridge-solana`, `validator`, `keeper`, `sig-store`, `graphql-api`, `indexer` |
| Solana | `crates/solana-gate` (excluded from the workspace, built with `cargo build-sbf`) |
| Frontend | all of `frontend/src` |
| Deployment | `Dockerfile`, `docker-compose.yml`, `.dockerignore`, `docker/configs/*.toml`, `docker/deploy.sh` |
| Scripts | `scripts/testing/*.sh` (29 files, headers and root resolution) |

What was verified by execution, not just by reading:

- **`forge test` passes: 99 tests, 8 suites, 0 failures.**
  Foundry 1.7.1, with `forge-std@v1.9.4` and `OpenZeppelin/openzeppelin-contracts@v5.0.2`.
  Breakdown: `Swap` 27, `Refund` 23, `Security` 20, `SwapRouter` 9, `Claim` 8, `SolanaBridge` 6, `Send` 5, `GenFixtures` 1.
- **`cargo test --workspace` passes: 40 tests, 0 failures.**
  `bridge-core` lib 18 and equivalence 1, `bridge-solana` e2e 2 and equivalence 3, `validator` 12 (5 state, 5 refund, 2 other), `sig-store` 4.
- **The cross-language hash equivalence holds.**
  `GenFixtures.t.sol` regenerates `contracts/fixtures/submission_ids.json` from Solidity, and the Rust `equivalence` suites consume it.
  Both sides pass against the same fixtures, and the fixture file was byte-identical after regeneration.
  This is the single most important invariant in the system and it is now confirmed from both directions.
- The frontend type-checks and builds cleanly.
  `tsc -b && vite build` in a scratch copy: 47 modules, 194 kB raw, 60.6 kB gzipped, zero errors under `strict: true`.
- All nine hardcoded function selectors in `frontend/src/wallet/eth.ts` are correct.
  Recomputed each one with Keccak-256 against the real signatures in the Solidity sources.
  Every one matches.
- The `u256_to_u64` saturation collision was reproduced in a scratch crate rather than inferred.
  `2^64-1`, `2^64`, and `2^200+7` all map to `18446744073709551615`.

What was not verified:

- No live chain, RPC, or database was exercised.
  The `scripts/testing/*.sh` end-to-end suite remains unrun, because those scripts are broken independently of tooling (see section 8.2).

---

## 2. Headline verdict

The core of this bridge is good work.

The trust boundary is drawn in the right place.
`bridge-core/src/store.rs` treats every inbound record as hostile, rebinds the id to its parameters, enforces parameter immutability, verifies the token binding against `debridgeId`, and guards `submission_id` against path traversal before it ever touches the filesystem.
It has nine tests named after the attacks they prevent.
The `submissionId` hashing is byte-identical between `BridgeHash.sol` and Rust, locked by fixture-based equivalence tests that I ran from both sides and confirmed agree.
The test suites are real and they pass: 99 Solidity tests and 40 Rust tests, zero failures.
`Refund.t.sol` alone has 23 tests, including replayed-signature-across-domains cases (`test_Cancel_RejectsReplayedRefundSignature`, `test_Refund_RejectsReplayedTransferSignature`) that prove the three signing prefixes actually provide the domain separation they claim to.
The refund path is genuinely two-phase and correctly ordered: validators only attest a refund after observing the destination burn on-chain, never on the database's say-so, and `refund.rs` has five tests that pin exactly that.
The keeper's cancel and refund paths deliberately discard their own transaction hashes and let the indexer record state from observed on-chain events, with a comment explaining why the keeper's word is not authoritative.
That is the right instinct, and it is written down.

The problems cluster in three places, and none of them are in the cryptography.

1. **The shipped deployment does not activate the features the code implements.**
   This is the dominant theme of the report and it recurs at three levels.
   The Docker image builds three of the eight binaries, and the indexer, which is the only writer of `refund_status`, is neither built nor deployed, so the entire refund lifecycle is a permanent no-op in the compose stack (H2).
   The `guardian` circuit breaker is implemented and tested, and `setGuardian` is never called anywhere in the repository, so it is `address(0)` in every possible deployment (M8).
   The `allow_zero_confirmation` safety key in all three shipped validator configs is not a field on the config struct and serde discards it in silence (M4).
   In each case the code is right and the wiring is missing.
2. **Failure handling advances state past errors.**
   Three separate loops treat a failed operation as a completed one and move the cursor forward.
   The affected work is never retried.
3. **The documentation is actively misleading.**
   `docs/BRIDGE_ARCHITECTURE.md` describes a NestJS/TypeORM implementation that does not exist anywhere in this repository.

Severity counts: 2 high, 8 medium, 13 low, plus a documentation hazard that outranks most of the code findings in practical cost.

Nothing here is a break in the cryptography, the hashing, or the signature verification.
The `submissionId` equivalence between Solidity and Rust is real and enforced, and I verified it from both directions.
The problems are in operations, failure handling, and wiring.

---

## 3. Critical: documentation describes software that does not exist

`docs/BRIDGE_ARCHITECTURE.md` (446 lines) describes a TypeScript validator built on NestJS with TypeORM entities, cron-driven scanners, a `buildSubmissionId.ts` module, Arweave uploads, and a `SelendraBridge_node/src/` source tree.

None of this exists.
The implementation is Rust, in `crates/`.
Sections 4 through 8 of that document are fiction end to end.

This is not a stale-docs nitpick.
A new engineer onboarding from this document will look for files that were never written, and will form a mental model of the trust boundaries that does not match the code.
Anyone auditing the system against this document is auditing nothing.

Treat this as the highest-priority item in the report.
It is also the cheapest to fix.

---

## 4. High severity

### H1. A reverted `claim()` is recorded as a successful one, permanently blocking the refund path

`crates/keeper/src/main.rs:393-405`

`try_claim` awaits the receipt, interpolates `receipt.status()` into a log line, and then returns `Ok(Some(tx_hash))` without ever branching on it.

```rust
let receipt = pending
    .with_timeout(Some(RECEIPT_TIMEOUT))
    .get_receipt()
    .await
    .context("await receipt")?;
info!(
    submission_id = %rec.submission_id,
    tx = %receipt.transaction_hash,
    status = receipt.status(),
    "CLAIMED"
);
Ok(Some(format!("{:#x}", receipt.transaction_hash)))
```

The caller at `main.rs:240-242` then runs `mark_claimed`, which sets `status = 'claimed'`.

Both `sweep_refund_eligible` (`bridge-db/src/lib.rs:774`) and `refund_candidates` (`:790`) filter on `status <> 'claimed'`.
Nothing ever resets that column back.

Failure scenario.
The keeper submits `claim()`, the transaction reverts on-chain for any reason (an out-of-gas estimate, a paused gate, a transient validator-set mismatch, insufficient gate liquidity in a race).
The receipt returns with `status = 0`.
The keeper logs `CLAIMED`, writes `status = 'claimed'`, and moves on.
The transfer was never delivered.
It is now permanently excluded from the refund sweep and from the refund candidate list.
The user's funds are locked on the source chain with no automated path to recovery.

This is worth fixing carefully, because the code already knows better.
The cancel and refund call sites at `:204-210` and `:323-330` explicitly discard their tx hashes with the comment "the keeper's word is not authoritative for a state that gates the refund-candidate list."
`mark_claimed` is the one place that violates the project's own stated rule, and the indexer already calls `mark_claimed` from the observed on-chain `Claimed` event at `indexer/src/main.rs:221`.

Recommended fix: check `receipt.status()` and return `Ok(None)` (or an error) on revert.
Consider deleting the keeper's `mark_claimed` call entirely and letting the indexer own that write, which restores the invariant the comments describe.

### H2. The shipped stack cannot perform a refund, because the indexer is not deployed

`Dockerfile:7`, `docker-compose.yml`

```dockerfile
RUN cargo build --release -p validator -p keeper -p sig-store
```

The image contains three binaries.
`indexer` and `graphql-api` are absent from the image and from compose.

The indexer is the sole writer of `refund_status`.
The keeper's refund loop gates on `refund_signatures.len() >= threshold`, those signatures are only produced by validators observing state the indexer records, and `sweep_refund_eligible` is only ever called from the indexer process.

Consequence: in the deployment the repository actually ships, no transfer ever becomes refund-eligible, no cancel is ever attested, and no refund is ever paid.
The refund feature is fully implemented, well tested, and completely unreachable.

The frontend is also absent from compose, and `graphql-api` is what it talks to, so the shipped stack has no user interface either.

Secondary issues in the same files:

- No `USER` directive.
  Both stages run as root.
- `docker-compose.yml` has no `restart:` policy on any service.
- All three validators mount the same `val-state` volume.
  The `state_file` paths are distinct (`/data/val1-state.json` and so on), so this is currently safe, but it is one config typo away from two validators sharing a nonce cursor.

---

## 5. Medium severity

### M1. `u256_to_u64` saturates, and the saturated value is used as a nonce-tracking key

`crates/validator/src/main.rs:399-401`

```rust
fn u256_to_u64(v: U256) -> u64 {
    v.try_into().unwrap_or(u64::MAX)
}
```

Used for `chainIdTo` (`:265`), `nonce` (`:266`), and `chainIdFrom` (`:311`).

`Gate.send` accepts `chainIdTo` as a full `uint256` with no upper bound.
Two sends with different `chainIdTo` values above `2^64-1` collapse to the same key in the validator's per-target-chain nonce map.

Failure scenario, two transactions.
Call `send` with `chainIdTo = 2^64-1` and let the validator run `accept_nonce(u64::MAX, 0)`.
Then call `send` with `chainIdTo = 2^64`.
It saturates to the same key while carrying contract nonce 0, so `check_nonce` returns `Duplicated` and pauses the scanner.
`/resume` re-reads the same log and pauses again immediately.
The validator is stuck until an operator manually advances the cursor.

Cost to an attacker: two ordinary `send` calls with a dust amount.

Fix: reject out-of-range values explicitly rather than saturating, and add a chain-id bound in `Gate.send`.

### M2. Three loops advance their cursor past work that failed

**Indexer** (`crates/indexer/src/main.rs:139-160`).
Three `scan` calls each log "will retry next tick" on error, and then `set_cursor(to_block)` runs unconditionally, followed by `from_block = to_block + 1`.
The retry the message promises never happens.
A transient RPC failure silently drops an entire block range: every `Sent`, `Claimed`, `Swapped`, and `Finalized` event in it is lost from the database forever.

**Validator batch** (`crates/validator/src/main.rs:225-244`).
An `Err` from `handle_log` is warned and skipped.
`paused` stays false, so `rt.persist.last_block = to_block` still advances.
A single transfer that fails to sign (a sig-store hiccup, for instance) is never signed and never revisited.

**Validator pause flag** (`crates/validator/src/state.rs:63,79,144`).
`paused` is a runtime-only field.
`load_or_init` hardcodes `paused: false`.
A validator that paused on a nonce anomaly and then restarts comes up unpaused, having forgotten the anomaly.
The safety property the pause exists to provide does not survive a process restart.

### M3. `verify_signature` does not check validator-set membership

`crates/bridge-core/src/store.rs:233-253`

The function recovers the address from the signature and confirms it matches the claimed signer.
It never checks that the signer is a member of the validator set.

Anyone who can reach the sig-store can therefore write well-formed signatures from arbitrary keys.
They inflate `signature_count`, they show up in the explorer UI as apparent attestations, and they pad the array the keeper submits.

This is not exploitable for fund loss, because `Gate._verifySignatures` counts only `isValidator[signer]` on-chain and enforces the threshold there.
It is still a real weakness: the off-chain view of "how many validators signed this" is attacker-controlled, the keeper wastes gas submitting padded arrays, and the operator dashboard can be made to lie.

The sig-store's bearer token is the only thing currently standing between an attacker and this.

### M4. Config typos are silently discarded, including the one the project relies on

`crates/validator/src/config.rs:93-115`

`SourceChain` has `#[serde(default)] pub block_confirmation: u64` with no validation, and no `#[serde(deny_unknown_fields)]`.

`docker/configs/val1.toml`, `val2.toml`, and `val3.toml` all contain:

```toml
block_confirmation = 0
allow_zero_confirmation = true   # anvil has instant finality; NEVER set on a real chain
```

`allow_zero_confirmation` is not a field on `SourceChain`.
Serde silently discards it.
The comment describes a safety guard that does not exist in the code.

So `block_confirmation = 0` is accepted on any chain, with no guard and no warning.
A validator pointed at a real chain with a zero finality buffer will sign transfers from blocks that can still be reorged.

Two fixes needed: add `#[serde(deny_unknown_fields)]` so typos become startup errors, and add an explicit `allow_zero_confirmation` opt-in that `block_confirmation == 0` actually requires.

### M5. `SwapPool.setPrice` deviation cap has no time gate

`contracts/src/SwapPool.sol:222-237`

The cap is per call.
There is no cooldown, no timestamp check, no cumulative window.

A compromised or buggy oracle key can walk the price arbitrarily far by making N successive calls, all within a single block.
With `maxPriceDeviationBps = 500` (5%), fourteen calls double the price.
The cap raises the gas cost of a manipulation and nothing else.

Fix: record `lastPriceUpdate` per token and require a minimum interval, or apply the cap against a time-weighted reference rather than the immediately preceding value.

### M6. The frontend can strand a cross-chain swap with no recovery path

`frontend/src/components/BridgeView.tsx:286-301`

After `swapAndBridge` succeeds, the component extracts the `Sent` event and stores everything needed for the later `finalize()` call in React state:

```tsx
const sent = extractSent(r.logs, gate);
if (!sent) throw new Error("Sent event not found in receipt — can't finalize automatically");
setPending({ submissionId: sent.submissionId, /* ... */ });
```

Two ways this strands a user, both after their funds have already left the source chain.

First, `extractSent` matches on the `gate` address the user typed into a text field.
In cross-swap mode the actual gate is `SwapRouter.gate`, a public immutable on the router.
If the user's typed gate does not match the router's gate, `extractSent` returns null, the error throws, `pending` is never set, and the UI offers no way forward.
The bridge transfer itself succeeded.

Second, and more likely in practice, `pending` lives only in component memory.
There is no `localStorage` anywhere in the frontend (verified).
A page refresh, a tab close, or a crash between `swapAndBridge` and `finalize` loses the finalize state permanently.
The stable arrives at the remote router and sits there.

`finalize` is permissionless, so the funds are not lost, but the UI provides no path to recover them and the user has no reason to know that.

The fix is cheap, because the data already exists.
`fetchHistory` already requests `swapIntent { tokenIn amountIn stableOut finalToken finalReceiver finalizeTx finalizeAmountOut finalizeFallback finalizedAt }` in `api/client.ts:88-91`, and `HistoryEntry` carries `debridgeId`, `amount`, `chainIdFrom`, `nonce`, and `receiver`.
Nothing in the UI renders any of it.
Rebuild `pending` from `history` on mount, and read the gate from `SwapRouter.gate()` instead of a text input.

### M7. `usePoll` has a request race that can show stale data as current

`frontend/src/api/hooks.ts:33-57`

`run()` never checks the `alive` flag; only the interval callback does.

```tsx
const run = useCallback(async () => {
  try {
    const d = await fnRef.current();
    setData(d);
    ...
```

When `deps` change, the previous effect's in-flight promise is not cancelled.
If it resolves after the new effect's fetch, it overwrites the new data with the old.

Failure scenario.
In the Explorer, change the corridor filter from "any" to a specific pair while the "any" request is slow.
The narrow result arrives, renders, and is then silently replaced by the full unfiltered list, under a filter UI that says otherwise.
The same applies to `SwapView` when switching chains: `pool` is not reset on a chain change, so the previous chain's token list stays on screen and drives `tokenIn` / `tokenOut` until the new fetch lands.

It also sets state after unmount, which is a React warning today.

Fix: capture a request id or an `AbortController` per run and drop results from superseded runs.

### M8. The circuit breaker is built, tested, and never wired up

`contracts/src/Gate.sol:40,227-246`

The `guardian` role is a well-designed low-trust stop button.
The comment explains the reasoning precisely: it can pause but never un-pause and never move funds, so a compromised guardian can only cause a denial of service.
`Security.t.sol` tests it from both directions with `test_Guardian_CanPauseButNotUnpause` and `test_SetGuardian_OnlyOwner`.

`setGuardian` is never called anywhere in the repository.
Grepped `contracts/script/`, `docker/`, and `scripts/`: zero hits.

So `guardian` is `address(0)` in every deployment this repo can produce, and only `owner` can trip the breaker.
In an incident, the response time of the entire bridge is bounded by how fast you can reach the owner key, which is exactly the key you most want to keep cold.
The whole point of a separate guardian is to let a hot, low-privilege key stop the bleeding while the cold key stays cold.

This is the same failure shape as H2: a correct, tested safety mechanism that no deployment path activates.
Worth checking the rest of the post-deploy wiring for the same pattern.

---

## 6. Low severity

### L1. `Dropdown` silently displays a value the parent is not holding

`frontend/src/components/Dropdown.tsx:23`

```tsx
const selected = options.find((o) => o.value === value) ?? options[0];
```

When `value` matches no option, the component renders `options[0]` and does not tell the parent.
The user sees one chain or token while the parent's state, which builds the transaction, holds another.

In `BridgeView` the destination-defaulting effect eventually corrects `toChainId`, so this is a window rather than a permanent divergence, but it is a window during which the UI misrepresents where funds are going.
Either render an explicit empty state or call `onChange(options[0].value)` so display and state cannot disagree.

### L2. Stale `decimals` during a token switch

`frontend/src/components/BridgeView.tsx:125-150`

`decimals` is not reset when `token` changes.
Between the change and the async `readDecimals` resolving, `amountBase = parseUnits(amount, decimals)` uses the previous token's decimals.

Switching from a 6-decimal token to an 18-decimal one and submitting quickly under-sends by 10^12.
The reverse direction over-sends, but the balance check catches that.
Reset `decimals` to null and disable the submit button until it resolves.

### L3. Transactions do not pin a chain id

`frontend/src/wallet/eth.ts:254-256`

```tsx
return (await req({ method: "eth_sendTransaction", params: [{ from, to, data }] })) as string;
```

No `chainId` field.
`BridgeView` derives `fromChainId` from `wallet.chainId` at render time and never re-checks it at submit time.
If the user switches networks between reading the form and confirming in the wallet, the transaction executes on the new chain against addresses meant for the old one.

Including `chainId` makes the wallet reject the mismatch instead.

### L4. `extractSent` matches by address only, not by event signature

`frontend/src/wallet/eth.ts:372-386`

The function takes the first log from the gate address with at least three topics.
It does not check `topics[0]` against the `Sent` signature hash (`0x8c7ee7a778ddf9672e509e70cf61fd826a6275ae6dd14c5e474b13898a1f2bbb`).

`Gate.send` currently emits exactly one event, so this is correct today (verified).
It breaks silently the moment another two-indexed event is added to that path, and the failure mode is a `finalize` built from garbage offsets rather than an error.

The word offsets themselves are correct: `Sent`'s non-indexed fields are `amount, chainIdFrom, chainIdTo, offset(receiver), nonce, ...`, so word 0 and word 4 are right.

### L5. `graphql-api` replays the full chain log history on every request

`crates/graphql-api/src/swap.rs:85-118`

`listed_tokens` issues two `get_logs` calls with `.from_block(0)` and no `to_block`, no pagination, and no caching.
This runs on every `pools()` query.

The frontend polls `fetchSwapPool` every 10 seconds, from `SwapView` and from both legs of `BridgeView`'s cross-swap quoting.
Against a real chain this is a self-inflicted denial of service on your own RPC provider, and most hosted providers will reject an unbounded `from_block(0)` filter outright.

Cache the token list and invalidate on `TokenListed` / `TokenDelisted`, or read it from the indexer's database.

### L6. `max_swap_usd` reports zero on overflow where the contract returns a real number

`crates/graphql-api/src/swap.rs`

```rust
reserve.checked_mul(price).map(|v| v / scale).unwrap_or(U256::ZERO)
```

The contract computes the same quantity with `Math.mulDiv`, which is 512-bit intermediate.
The Rust version overflows first and reports `0`, which the UI renders as "no capacity available" for a pool that in fact has plenty.

Use a widening multiply or mirror `mulDiv`.

### L7. `crates/solana-gate` must not ship as written

The program is deployable BPF, and it is not on the live path today: it is `exclude`d from the workspace, absent from compose, and its `sol_log_data` event format is incompatible with `relayer.rs::parse_sent_log_line`.
That correctly downgrades it from a live vulnerability to a landmine.

The problems, so they are recorded before someone deploys it:

- No PDA or owner validation on `config_ai` outside `process_init`.
  Any account can be passed as config.
- No asset registry and no liquidity checks.
- `emit_sent` (`lib.rs:485-492`) writes 64 bytes and discards the rest with `let _ = (id, chain_id_from, nonce, native_sender);`.
  The emitted event does not contain the data a validator needs.

Notably, the reference model in `crates/bridge-solana/src/gate.rs` is *more* correct than the deployable program: it has `token_of`, `vault`, `UnknownAsset`, and `InsufficientLiquidity`.
The two have diverged, and the tested one is not the deployable one.

### L8. The Solana host model diverges from real revert semantics

`crates/bridge-solana/src/gate.rs:209-217`

`claim()` inserts into `executed` before the asset and liquidity checks.
In the host model a failed claim therefore permanently burns the `submissionId`.
On a real Solana program the whole instruction would roll back and the id would remain claimable.

`tests/e2e.rs` tests against this model, so the test suite is currently asserting the wrong semantics.

### L9. `_verifySignatures` loop is unbounded

`contracts/src/Gate.sol:524-542`

`signatures.length` is not capped.
An attacker can pad the array with valid non-validator signatures in ascending address order.

Ranked low deliberately.
`claim` is permissionless and the attacker pays their own gas, so this grieves nobody but themselves and cannot block another keeper.
A cap is still cheap insurance against an accidental gas-limit lockout with a large validator set.

### L10. Secrets reach the Docker build context

`.dockerignore` excludes `validator*.toml` and `keeper*.toml`, but not `docker/configs/*.toml`, which contain inline `private_key` values.
`COPY . .` therefore bakes them into the builder layer.

To be precise about the blast radius: the final image only copies the three binaries, so the keys are not in the runtime image.
They are in the build cache, and in any image pushed with `--target builder`.
These are anvil's well-known development keys, so nothing is currently at risk.
The pattern is what needs fixing before a real key is ever placed there.

Also missing from `.dockerignore`: `frontend/node_modules`, `frontend/dist`, and (after the restructure below) `docs`.

### L11. GraphQL arguments are inlined as query-string literals

`frontend/src/api/client.ts:98-127`

`chainId` and `limit` are interpolated into the query text rather than passed as variables, with a comment explaining that the `u64` scalar has no convenient variable name.

The values come from a dropdown and a hardcoded `100`, so this is not currently exploitable.
`Number(from)` producing `NaN` would emit a syntactically broken query.
Guard with `Number.isInteger` at minimum, and prefer a proper scalar so variables can be used.

### L12. Test coverage cannot be measured, because `forge coverage` does not run

`forge coverage` fails to compile this project.

```
Error: Compiler error: Stack too deep.
   --> src/Gate.sol:308:13
```

Coverage disables the optimizer and `viaIR` to keep source mappings accurate, and `Gate.send`'s ten-field `emit Sent(...)` at `Gate.sol:298-309` exceeds the stack limit without them.
`--ir-minimum`, the documented workaround, also fails: `Cannot swap Variable value15 with Slot 0x01: too deep in the stack by 1 slots`.

The contracts build and test fine under the project's real settings (`via_ir = true`, `optimizer = true`), so this is not a production problem.
It does mean nobody on the team can produce a coverage report, and has not been able to.

That is not academic.
Section 9 item 19 lists two uncovered branches (M1 and M5) that I found by reading test names by hand.
A coverage report would have surfaced those, and will surface the ones I missed.

Fix: reduce stack pressure at the emit site, most simply by scoping the local variables in a block, or by packing the event arguments through a memory struct.
One targeted change to `Gate.send` buys the team a permanent coverage capability.

### L13. `DeployXSwap.s.sol` deploys a threshold-1 gate, with no guard against pointing it at a real network

`contracts/script/DeployXSwap.s.sol:52-54`

```solidity
address[] memory vals = new address[](1);
vals[0] = validator;
Gate gate = new Gate(vals, 1);
```

One validator, threshold one.
A single signature releases funds.

The same script deploys `XMintable` tokens whose `mint` is unrestricted (anyone can mint any amount) and sets `DEVIATION_BPS = 1000`, a 10% per-update price cap which, combined with M5's missing time gate, lets an oracle double a price in eight calls within one block.

All of this is correct for a local demo, which is plainly what it is for.
The concern is that nothing stops it running elsewhere: no `block.chainid` assertion, no `require` on a known-local chain id, and the filename does not say "local".
`DeploySwap.s.sol` has the same shape.

`docker/deploy.sh` is better and does the right thing: three validators, threshold two, plus a sanity check that the deployed addresses match the baked configs.

Add a chain-id guard to both scripts.
It is two lines and it removes a whole category of very bad afternoon.

Separately worth noting: there is **no reviewed deploy path for a production Gate**.
The only Gate deployments in the repository are this demo script and a `forge create` line in `docker/deploy.sh`.
A real deployment will be somebody typing a command, with no script, no checklist, and no post-deploy verification of the validator set, the threshold, the guardian (M8), or the asset registry.

---

## 6b. What came back clean

Recording these so the team knows they were checked and are not open questions.

**Contract sizes are comfortable.**
Nothing is near the 24,576-byte EIP-170 limit.

| Contract | Runtime | Margin |
| --- | --- | --- |
| SwapRouter | 6,507 B | 18,069 B |
| Gate | 5,983 B | 18,593 B |
| SwapPool | 5,739 B | 18,837 B |

There is ample room to add the guards recommended in this report without size pressure.

**The production contracts compile warning-free.**
`forge build --force` emits exactly two warnings, both "state mutability can be restricted to pure" on test helpers (`Claim.t.sol:70`, `Refund.t.sol:103`).
Zero warnings in `src/`.

**Clippy is nearly clean.**
`cargo clippy --workspace --all-targets` produces 11 warnings, all cosmetic: three `too_many_arguments` from the generated alloy `sol!` bindings in `abi.rs`, two doc-indentation nits, two `useless_vec` in tests, one `redundant_clone`, one elidable lifetime, one `to_vec`.
No correctness lints anywhere.
`crates/solana-gate` adds 4 more.

Per the project's own rule that clippy warnings are real issues to be fixed rather than suppressed, these are worth an afternoon, but none of them indicate a bug.

**The hashing equivalence is genuinely locked.**
I regenerated `fixtures/submission_ids.json` from Solidity via `GenFixtures.t.sol` and the file came back byte-identical, then confirmed the Rust `equivalence` suites still pass against it.
The claim in the README that the two implementations are byte-for-byte identical is true, and it is enforced, not asserted.

**No XSS or injection surface in the frontend.**
No `dangerouslySetInnerHTML`, `innerHTML`, or `eval`.
`window.open` uses `noopener`.

---

## 7. Frontend review

This section is the developer-facing feedback on `frontend/`, separate from the severity list above.

### What is good

The decision to hand-roll ABI encoding instead of pulling in ethers or viem is defensible here, and it was executed carefully.
Every call site uses only static `address` and `uint256` arguments plus dynamic `bytes`, the offset arithmetic in `encodeSend`, `encodeAutoParamsTo`, and `encodeFinalize` is correct, and all nine selectors verify against the real contract signatures.
The result is a 60 kB gzipped bundle for a working bridge UI, which is a genuinely good outcome.

Wallet handling in `useWallet.ts` is better than most production dApps.
It handles the EIP-6963 announce race, MetaMask's legacy `ethereum#initialized` event, and a bounded 5-second poll, any of which flips detection on reactively.
Error code 4001 and -32002 are both mapped to human messages.
This is the kind of detail that usually only gets added after a support ticket.

The comments explain *why*, not *what*.
`BridgeView.tsx:81-85` on why the destination default is skipped while a transfer is in flight, and `:607-611` on why the corridor warnings are hidden once the flow leaves idle, both document reasoning a future reader could not reconstruct from the code.
`useChainDecimals.ts` is honest that it is a heuristic and says exactly when it is wrong.
Keep doing this.

Security hygiene is clean.
No `dangerouslySetInnerHTML`, no `innerHTML`, no `eval`, no `localStorage`, and `window.open` uses `noopener`.
`strict: true` with `noUnusedLocals` and `noUnusedParameters`, and the build is warning-free.

### What needs work

**No tests, at all.**
This is the most important gap in the frontend, and it is the top recommendation in section 9.

The functions that most need tests are pure and trivial to test: `encodeSend`, `encodeFinalize`, `encodeAutoParamsTo`, `encodeSwapIntent`, `extractSent`, `parseUnits`, `formatUnits`, `formatUnitsRaw`.
A single wrong byte offset in `encodeFinalize` sends a user's funds to the wrong address, and nothing in the repository would catch it.
`bun test` with fixture vectors taken from `cast abi-encode` would take an afternoon and would meaningfully de-risk the highest-consequence code in the frontend.

**No lint configuration.**
There are `eslint-disable-next-line react-hooks/exhaustive-deps` comments in five files, but no `eslint.config.mjs` and no eslint dependency.
Those suppressions are currently suppressing nothing.
Several of them are legitimate and deliberate; they should be enforced by a linter that actually runs, not documented against one that does not exist.

**No error boundary.**
A throw anywhere in `BridgeView`, `SwapView`, or `Explorer` blanks the entire page.
Given that this UI moves money, a boundary that preserves the wallet connection and shows a recoverable error is worth the twenty lines.

**Accessibility gaps.**
These are real for a financial interface:

- `Dropdown` has `role="listbox"` and `role="option"` but no `aria-expanded` or `aria-haspopup` on the trigger, and no arrow-key navigation.
  It is mouse-only.
- `SubmissionDetail`'s drawer has `role="dialog"` but no `aria-modal`, no focus trap, and no focus restoration on close.
  Escape is handled, which is good.
- Explorer table rows are `<tr onClick>` with no `tabIndex` or key handler, so the detail drawer is unreachable by keyboard.
- The scrim is a `div` with `onClick` and no keyboard equivalent.

**Package manager violation.**
`frontend/package-lock.json` is committed and there is no `bun.lock`.
Per the project rules this should be bun-only.
Delete the lockfile, run `bun install`, and commit `bun.lock`.

**Minor observations:**

- `App.tsx:12` defaults to the `swap` view while `Navbar` lists Bridge first.
  Pick one.
- `parseUnits` silently truncates excess precision (`format.ts:55`) and returns `0n` for invalid input rather than signalling.
  The Max button uses `formatUnitsRaw`, which is full precision, so Max is exact.
  Truncation is defensible; silently returning `0n` for a typo is worth a visible validation state.
- `sendApprove` approves exactly `amountBase` rather than an unlimited allowance.
  That is the right call and worth keeping.
  Note that USDT-style tokens requiring a reset-to-zero before re-approval will revert; worth handling if such a token is ever listed.
- `SwapView`'s `exceedsLock` check uses pool data that is up to 10 seconds stale.
  The contract is authoritative, so this is a UX hint rather than a correctness issue, but the error message should say so.
- `vite.config.ts` proxies `/graphql` and `/health` in dev only.
  Production requires same-origin serving or `VITE_API`.
  This is documented in `api/client.ts:1-3` but not in the deployment path, and there is no production serving story in compose at all.

---

## 8. Plan 1: restructure the repository

### 8.1 Why

The `bridge/` directory contains the entire project.
The root contains a one-line README and a `docs/` folder that documents software that was never written.
Every path in every script, every Docker context, and every editor session carries a level of nesting that buys nothing.

There is also a concrete bug that the restructure should fix along the way.

### 8.2 Pre-existing bug found while planning this

**All 20 test scripts are broken right now.**

```bash
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS="$ROOT/contracts"
```

The scripts live in `bridge/scripts/testing/`, so `dirname/..` resolves to `bridge/scripts`, and `$ROOT/contracts` becomes `bridge/scripts/contracts`, which does not exist.
Verified: zero of the 20 scripts use `../..`, and `bridge/scripts/contracts` is absent.

`e2e.sh`'s own header still says `Run from anywhere: bash scripts/e2e.sh`, and `bridge/README.md` still documents the tree as `scripts/e2e.sh`.
The scripts were moved into a `testing/` subdirectory and nothing was updated.

This must be fixed regardless of the restructure.
Usefully, the same fix is correct both before and after the move: `"$(dirname "${BASH_SOURCE[0]}")/../.."` resolves to `bridge/` today and to the repo root afterwards.
So fix it first, and the move itself needs no further script churn.

### 8.3 Target structure

```
selendrabridge/
├── README.md                  # from bridge/README.md, tree corrected
├── report.md                  # this document
├── Cargo.toml                 # workspace root, now at repo root
├── Cargo.lock
├── Dockerfile
├── docker-compose.yml
├── .dockerignore
├── .gitignore                 # merged, bridge/ prefixes stripped
├── contracts/                 # Foundry: Gate, BridgeHash, SwapPool, SwapRouter
│   ├── src/  test/  script/  fixtures/
├── crates/
│   ├── bridge-core/           # the sacred hashing + store (trust boundary)
│   ├── bridge-db/             # Postgres via sqlx
│   ├── bridge-solana/         # host-side reference model
│   ├── validator/             # scan -> recompute -> sign -> store
│   ├── keeper/                # collect >= threshold -> claim / cancel / refund
│   ├── sig-store/             # axum signature store
│   ├── indexer/               # sole writer of refund_status
│   ├── graphql-api/           # read API for the frontend
│   └── solana-gate/           # excluded from the workspace, cargo build-sbf
├── frontend/                  # React 18 + Vite + TS
├── docker/                    # compose configs + deploy helper
├── scripts/
│   └── testing/               # 29 e2e and integration scripts
└── docs/
    ├── README.md              # index: what to read, in what order
    ├── architecture.md        # WRITTEN, from the code
    ├── operations.md          # TODO: running the stack, configs, secrets, deploy checklist
    └── history/
        ├── bridge-build-plan.md
        └── swap-build-plan.md
```

`docs/` holds only cross-cutting material.
`contracts/README.md`, `frontend/README.md`, and `crates/solana-gate/README.md` stay next to their code; see section 8.6.

### 8.4 Execution, as three commits

**Commit 1: fix the broken script root resolution.**
Independent of the move, and it makes the move commit pure.

```bash
cd bridge
sed -i 's|}")/\.\." && pwd)|}")/../.." \&\& pwd)|' scripts/testing/*.sh
sed -i 's|dirname "$0")/\.\."|dirname "$0")/../..|' scripts/testing/solana-localnet-e2e.sh
```

Then verify by hand that `$ROOT` prints the `bridge/` directory in at least three scripts before committing.
Note that `solana-localnet-e2e.sh` uses `$0` rather than `BASH_SOURCE`, so it needs the separate pass shown above.

**Commit 2: the move, `git mv` only, no content edits.**
Keeping this commit pure means `git log --follow` and rename detection work cleanly afterwards.

```bash
git mv bridge/Cargo.toml bridge/Cargo.lock bridge/Dockerfile bridge/docker-compose.yml .
git mv bridge/.dockerignore .
git mv bridge/contracts bridge/crates bridge/docker bridge/frontend bridge/scripts .
git mv bridge/README.md docs/_incoming-readme.md      # resolved in commit 3
git mv bridge/.gitignore docs/_incoming-gitignore     # resolved in commit 3
```

`bridge/target/` is untracked and will be left orphaned.
Removing it is a destructive operation on 4+ GB of build cache, so it is deliberately not scripted here.
Delete it manually once the workspace rebuilds cleanly at the new root.

**Commit 3: fix every path the move invalidated.**

| File | Change |
| --- | --- |
| `.gitignore` | Merge the two files, strip the `bridge/` prefix from the 5 rules that carry it (`bridge/frontend/src/data/` negations, `bridge/contracts/fixtures/*`, `bridge/.xswap-logs`) |
| `.dockerignore` | Add `docs`, `report.md`, `frontend/node_modules`, `frontend/dist`, `docker/configs/*.toml` |
| `README.md` | Replace the stub with the incoming one; correct the tree (drop the `bridge/` root, add `indexer`, `graphql-api`, `bridge-db`, `bridge-solana`, `frontend`, fix `scripts/e2e.sh` to `scripts/testing/e2e.sh`); repoint `../docs/` links to `docs/` |
| `crates/solana-gate/README.md:15` | `bridge/Cargo.toml` becomes `Cargo.toml` |
| `scripts/testing/web-smoke.sh:2` | `bridge/web` becomes `frontend` |

`docker-compose.yml` needs no change: `build: .` and `./docker/configs` are both relative to the compose file, which moves with them.
`Cargo.toml` workspace members are relative and unaffected.

### 8.5 Verification gate

Do not consider the restructure done until all of these pass.

```bash
cargo build --workspace && cargo test --workspace
cargo clippy --workspace --all-targets
cd frontend && bun install && bun run build && cd ..
docker compose config > /dev/null
bash scripts/testing/e2e.sh          # requires foundry
git grep -n "bridge/" -- ':!docs/history'   # should return only intentional hits
```

The `e2e.sh` run is the one that actually proves commit 1 worked.
It needs Foundry, which is not currently installed on this machine.

### 8.6 Docs merge

Current inventory and disposition:

| File | Lines | Assessment | Action |
| --- | --- | --- | --- |
| `docs/BRIDGE_ARCHITECTURE.md` | 446 | **Fiction.** Describes NestJS, TypeORM, `SelendraBridge_node/src/`, Arweave. Sections 4-8 have no counterpart in the codebase. | **Do not migrate.** Delete, and write `docs/architecture.md` from the Rust sources. |
| `docs/BRIDGE_BUILD_PLAN.md` | 403 | Historical phase plan, largely realised. Genuinely useful as a record of intent. | `docs/history/bridge-build-plan.md`, with a header noting it is historical. |
| `docs/SWAP_BUILD_PLAN.md` | 377 | Historical. Still says "This is a plan; no contract code is written yet", which is no longer true. | `docs/history/swap-build-plan.md`, same header treatment. |
| `bridge/README.md` | 318 | Accurate on the hashing and architecture. Tree is stale: omits `indexer`, `graphql-api`, `bridge-db`, `bridge-solana`, `frontend`, and lists `scripts/e2e.sh`. | Becomes root `README.md`, tree corrected. |
| `contracts/README.md` | 66 | Accurate. | **Stays in `contracts/`.** |
| `frontend/README.md` | 75 | Accurate. | **Stays in `frontend/`.** |
| `crates/solana-gate/README.md` | 53 | Accurate, and carries an important warning about workspace exclusion. | **Stays in `crates/solana-gate/`.** |

**Decision: component READMEs stay where they are.**

`docs/` becomes the single home for cross-cutting documentation, the material that describes how the pieces fit together and belongs to no one component.
Documentation that describes one directory stays in that directory.

The `crates/solana-gate/README.md` case is the clearest argument for this.
Someone who lands in that directory needs to learn that it is excluded from the workspace and unsafe to deploy as written.
That warning has to be where they will actually be standing, not filed under a path they would have to already know about.
The same reasoning applies to `contracts/` and `frontend/`: a README next to the code is discovered by anyone who opens the directory, and a README in `docs/` is discovered only by someone who already went looking.

`docs/architecture.md` cross-links to all three rather than absorbing them.

**Status: `docs/architecture.md` is written** (section 10 of that document lists a suggested reading order).
It covers the three signing prefixes and the domain separation they provide, both `submissionId` preimages and where the equivalence is enforced, the trust boundary at `bridge-core/src/store.rs` guard by guard, the two-phase refund and why validators re-verify the burn on-chain instead of trusting the database, and a table of which processes are required for which features, since H2 shows that was not obvious to anyone, including whoever wrote the `Dockerfile`.

It also carries inline operational warnings at each point where a known defect from this report would bite a reader, cross-referenced by finding id, so someone reading the architecture doc cannot walk into H1, H2, M2, M3, M4, M5, M8, L7, L8, or L12 unaware.
Those warnings should be deleted as the corresponding items are fixed.

Remaining doc work after the move:

- Delete `docs/BRIDGE_ARCHITECTURE.md`.
- Move the two build plans to `docs/history/` with a header marking them historical.
- Write `docs/operations.md`: running the stack, configuring a validator, key handling, and a real deploy checklist (which does not currently exist anywhere, see L13).
- Write `docs/README.md` as an index.

---

## 9. Next TODO

Ordered by consequence divided by effort.

### Do before any further deployment

1. **Check `receipt.status()` in `try_claim`.**
   H1.
   One-line fix for a permanent fund-stranding bug.
   Consider deleting the keeper's `mark_claimed` call entirely and letting the indexer own that write, which restores the invariant the surrounding comments already describe.
2. **Add `indexer` and `graphql-api` to the Dockerfile and to compose.**
   H2.
   Without this the refund feature, which is fully built and well tested, does not exist in production.
   Add a `USER` directive and `restart: unless-stopped` in the same pass.
3. **Rewrite or delete `docs/BRIDGE_ARCHITECTURE.md`.**
   Section 3.
   It is currently worse than having no documentation.
4. **Set a guardian, and write a real deploy checklist.**
   M8 and L13.
   The circuit breaker exists and is tested, and no deployment this repository can produce has it enabled.
   There is also no reviewed deploy path for a production Gate at all, only a demo script and a `forge create` line.
   Whatever a mainnet deploy looks like, it should be a script with post-deploy assertions on the validator set, the threshold, the guardian, and the asset registry.

### Do in the next sprint

5. **Stop advancing cursors past failures.**
   M2, all three sites.
   The indexer one is the worst: it permanently drops entire block ranges from the database on a transient RPC error.
6. **Persist the validator `paused` flag.**
   M2.
   A safety pause that a restart clears is not a safety pause.
7. **Add `#[serde(deny_unknown_fields)]` to the validator config, and make `block_confirmation = 0` require an explicit opt-in.**
   M4.
   The `allow_zero_confirmation` key in all three shipped configs is currently discarded in silence.
8. **Reject out-of-range `U256` values instead of saturating, and bound `chainIdTo` in `Gate.send`.**
   M1.
9. **Check validator-set membership in `verify_signature`.**
   M3.
10. **Fix the frontend request race in `usePoll`.**
    M7.
    Add an abort or a request-id guard, and reset `data` on a deps change.
11. **Make the cross-chain swap recoverable.**
    M6.
    Read the gate from `SwapRouter.gate()` instead of a text input, and rebuild `pending` from the `history` query, whose `swapIntent` payload is already being fetched and thrown away.
12. **Make `forge coverage` runnable, then look at what it says.**
    L12.
    Reducing stack pressure at `Gate.sol:308` is a small change that unlocks a capability the team currently does not have.
    Do this before item 19, because it will tell you which other branches are untested.
13. **Add a `block.chainid` guard to both deploy scripts.**
    L13.
    They deploy threshold-1 gates and unrestricted-mint tokens, and nothing stops them running against a real network.

### Do when convenient

14. **Add frontend tests.**
    Section 7.
    `bun test` over `encodeSend`, `encodeFinalize`, `encodeAutoParamsTo`, `extractSent`, and `parseUnits`, with fixture vectors from `cast abi-encode`.
    This is the highest-value test work available anywhere in the repository right now: the code is pure, the failure mode is silent fund misdirection, and the current coverage is zero.
15. **Close the two contract test-coverage gaps this review exposed.**
    The 99-test Solidity suite is strong, but it does not cover either of the contract-level findings above.
    `Swap.t.sol` has `test_SetPrice_OracleWithinDeviation` and `test_SetPrice_RevertsDeviationTooHigh`, both single-call, and nothing that walks the price across repeated calls (M5).
    No test anywhere passes an out-of-range `chainIdTo` to `Gate.send` (M1).
    Add one test for each; both are a few lines and both would have caught the bug.
16. **Fix the script root resolution.**
    Section 8.2.
    All 20 test scripts are broken today.
    Do this as commit 1 of the restructure.
17. **Add `eslint.config.mjs`.**
    There are five `eslint-disable` comments enforcing nothing.
18. **Add a time gate to `SwapPool.setPrice`.**
    M5.
19. **Cache `listed_tokens` in `graphql-api`.**
    L5.
    An unbounded `from_block(0)` on a 10-second poll will be rejected by most hosted RPC providers.
20. **Reconcile `solana-gate` with `bridge-solana`, or mark it clearly unsupported.**
    L7, L8.
    The tested model and the deployable program have diverged, and the deployable one is the weaker of the two.
21. **Frontend accessibility and an error boundary.**
    Section 7.
22. **Clear the 11 clippy warnings.**
    Section 6b.
    All cosmetic, none indicate a bug, but the project's own rule is to fix rather than suppress them.
23. **Delete `frontend/package-lock.json`, commit `bun.lock`.**

---

## 10. Appendix: verification log

| Claim | How it was checked | Result |
| --- | --- | --- |
| Solidity test suite passes | `forge test`, Foundry 1.7.1, OZ v5.0.2, forge-std v1.9.4 | 99 tests, 8 suites, 0 failures |
| Rust test suite passes | `cargo test --workspace` | 40 tests, 0 failures |
| Cross-language hash equivalence | `GenFixtures.t.sol` regenerates the fixtures, Rust `equivalence` suites consume them | Both sides pass, fixture file byte-identical after regeneration |
| M5 has no test coverage | Listed all `setPrice` test names | Only single-call deviation is tested |
| M1 has no test coverage | Grepped the suite for `chainIdTo` bounds | No test passes an out-of-range value |
| Frontend builds under `strict` | `tsc -b && vite build` in a scratch copy | Clean, 194 kB / 60.6 kB gzipped |
| All 9 frontend selectors correct | Keccak-256 recomputed against the Solidity signatures | 9/9 match |
| `extractSent` word offsets correct | Read `Sent`'s field order in `Gate.sol:79-90` | word 0 = amount, word 4 = nonce, correct |
| `Gate.send` emits one event | Read the function body | One `emit Sent` |
| `u256_to_u64` collision | Scratch crate, three inputs | All map to `u64::MAX` |
| Test scripts broken | 0/20 use `../..`; `bridge/scripts/contracts` absent | Confirmed broken |
| Keeper ignores receipt status | 3 `get_receipt()` sites, `status` appears once, in a log string | Confirmed |
| No XSS vectors in frontend | Grepped `dangerouslySetInnerHTML`, `innerHTML`, `eval` | None |
| No client-side persistence | Grepped `localStorage`, `sessionStorage` | None, which is why M6 strands users |
| Contracts compile warning-free | `forge build --force` | 2 warnings, both on test helpers, none in `src/` |
| Contract sizes under EIP-170 | `forge build --sizes` | Largest is SwapRouter at 6,507 B, 18 KB of margin |
| Clippy | `cargo clippy --workspace --all-targets` | 11 warnings, all cosmetic, no correctness lints |
| Guardian never wired | Grepped `setGuardian` in `script/`, `docker/`, `scripts/` | Zero hits, so it is `address(0)` everywhere |
| Coverage cannot be measured | `forge coverage`, then `--ir-minimum` | Both fail with stack-too-deep at `Gate.sol:308` |
| End-to-end shell suite | Not run | **Unverified**, the scripts are broken (section 8.2) |
