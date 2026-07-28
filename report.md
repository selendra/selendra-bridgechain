# SelendraBridge Status

Review date: 2026-07-28.
Last updated: 2026-07-28, after the restructure.
Source of truth is the code.

---

## Where we are

The project now lives at the repo root.
`bridge/` is gone except for an orphaned `target/` build cache that still needs deleting by hand.

The cryptography is sound and locked by tests.
Nothing in this report is a break in the hashing, the signature verification, or the trust boundary.

Two high-severity defects are open, and both are wiring, not logic.
A reverted `claim()` is recorded as successful (H1), and the shipped Docker stack cannot perform a refund because the indexer is not in it (H2).

Start at H1.

---

## Done

| Commit | What |
| --- | --- |
| `226a66f` | Fixed 22 test scripts whose `$ROOT` resolved one level short. Also `$ROOT/web` to `frontend/`, and the `allowlist.sh` cross-reference. |
| `11bda89` | Moved the project out of `bridge/` to the repo root. 139 files, pure `git mv`. |
| `ea8de0c` | Merged `.gitignore`, extended `.dockerignore`, corrected the README tree, merged the docs. |
| `4231d0c` | Restored `_detect_ed2024.py`, `tools/localnet/`, and `chains.json`, deleted in `525b109` while still referenced. |

Findings closed by the above:

- **Section 3, doc fiction.** `docs/BRIDGE_ARCHITECTURE.md` deleted. `docs/architecture.md` written from the sources. `docs/operations.md` and `docs/README.md` are new. Build plans moved to `docs/history/`.
- **L10, secrets in the build context.** `.dockerignore` now excludes `docker/configs/*.toml`, plus `frontend/node_modules`, `frontend/dist`, `docs`, `report.md`.
- **Section 8.2, broken scripts.** All root resolution fixed.
- **TODO 23, package manager.** `frontend/package-lock.json` replaced by `bun.lock`. Scripts use `bun`/`bunx`.

Correction to the original review:

- **M4 was half wrong.** `allow_zero_confirmation` is a real, enforced field on `[refund]`, and `Config::load` refuses to start at buffer 0 without it. Only the `[source]`-level key is silently discarded. Documented in `docs/operations.md` §3.1.

---

## Open

### High

**H1. A reverted `claim()` is recorded as successful.**
`crates/keeper/src/main.rs:393-405`.
`try_claim` logs `receipt.status()` but never branches on it, then returns `Ok`, and the caller runs `mark_claimed`.
`sweep_refund_eligible` (`bridge-db/src/lib.rs:774`) and `refund_candidates` (`:790`) both filter `status <> 'claimed'`, and nothing resets the column.
A failed delivery is permanently excluded from refund.
Fix: branch on `receipt.status()`, or delete the keeper's `mark_claimed` and let the indexer own that write from the observed `Claimed` event (`indexer/src/main.rs:221`).

**H2. The shipped stack cannot refund.**
`Dockerfile:7` builds `validator`, `keeper`, `sig-store` only.
`indexer` is the sole writer of `refund_status` and is absent from the image and from compose, so no transfer ever becomes refund-eligible.
`graphql-api` and the frontend are also absent, so the stack has no UI.
Fix: add both to the Dockerfile and compose. Add a `USER` directive and `restart: unless-stopped` in the same pass.

### Medium

| Id | Location | Defect |
| --- | --- | --- |
| M1 | `validator/src/main.rs:399` | `u256_to_u64` saturates to `u64::MAX`. Two sends with `chainIdTo` above `2^64-1` collide in the nonce map and wedge the scanner. Costs an attacker two dust sends. Bound `chainIdTo` in `Gate.send` too. |
| M2 | `indexer/src/main.rs:139-160` | Cursor advances past a failed scan. A transient RPC error drops an entire block range from the DB permanently. |
| M2 | `validator/src/main.rs:225-244` | Same shape. A log that fails to sign is never revisited. |
| M2 | `validator/src/state.rs:63,79,144` | `paused` is runtime-only. A validator that paused on an anomaly comes back unpaused after restart. |
| M3 | `bridge-core/src/store.rs:233-253` | `verify_signature` does not check validator-set membership. Not fund-loss (the Gate counts `isValidator` on-chain), but the off-chain signature count is attacker-controlled. |
| M4 | `validator/src/config.rs:93-115` | `SourceChain` has no `deny_unknown_fields` and no `allow_zero_confirmation`. `block_confirmation = 0` is accepted on any chain with no guard. |
| M5 | `contracts/src/SwapPool.sol:222-237` | `setPrice` deviation cap is per call with no cooldown. N calls in one block walk the price N times. |
| M6 | `frontend/src/components/BridgeView.tsx:286-301` | Cross-chain swap strands with no recovery. `pending` lives only in component memory, and `extractSent` matches the user-typed gate rather than `SwapRouter.gate()`. Data to rebuild it is already fetched in `api/client.ts:88-91` and discarded. |
| M7 | `frontend/src/api/hooks.ts:33-57` | `usePoll`'s `run()` ignores the `alive` flag, so a superseded request can overwrite current data. |
| M8 | `contracts/src/Gate.sol:40,227-246` | `setGuardian` is called nowhere in the repo, so `guardian` is `address(0)` in every deployment. Only `owner` can trip the breaker. |

### Low

| Id | Location | Defect |
| --- | --- | --- |
| L1 | `frontend/src/components/Dropdown.tsx:23` | Falls back to `options[0]` without telling the parent. Display and state can disagree. |
| L2 | `frontend/src/components/BridgeView.tsx:125-150` | `decimals` not reset on token change. A fast submit mid-switch under-sends by `10^12`. |
| L3 | `frontend/src/wallet/eth.ts:254-256` | `eth_sendTransaction` pins no `chainId`. A network switch mid-flow executes against the wrong chain's addresses. |
| L4 | `frontend/src/wallet/eth.ts:372-386` | `extractSent` matches by address only, not `topics[0]`. Correct today; silent garbage the moment a second two-indexed event joins that path. |
| L5 | `graphql-api/src/swap.rs:85-118` | `listed_tokens` does two `from_block(0)` `get_logs` per `pools()` query, on a 10s frontend poll. Most hosted RPCs will reject it. |
| L6 | `graphql-api/src/swap.rs` | `max_swap_usd` returns `0` on overflow where the contract's `Math.mulDiv` returns a real number. UI shows "no capacity" for a funded pool. |
| L7 | `crates/solana-gate` | No PDA/owner validation on `config_ai` outside `process_init`, no asset registry, no liquidity checks. `emit_sent` discards the data a validator needs. Excluded from the workspace and not on the live path, so it is a landmine, not a live hole. |
| L8 | `bridge-solana/src/gate.rs:209-217` | Host model inserts into `executed` before the asset checks, so a failed claim burns the id. Real Solana would roll back. `tests/e2e.rs` asserts the wrong semantics. |
| L9 | `contracts/src/Gate.sol:524-542` | `signatures.length` uncapped. Self-griefing only, since `claim` is permissionless and the attacker pays. |
| L11 | `frontend/src/api/client.ts:98-127` | GraphQL args inlined as query-string literals. Not exploitable today (dropdown + hardcoded `100`), but `NaN` emits a broken query. |
| L12 | `contracts/src/Gate.sol:308` | `forge coverage` fails with stack-too-deep, `--ir-minimum` too. Nobody can produce a coverage report. Scope the locals at the `emit Sent` site. |
| L13 | `contracts/script/DeployXSwap.s.sol:52-54` | Deploys a threshold-1 gate and unrestricted-mint tokens with no `block.chainid` guard. `DeploySwap.s.sol` same. There is also no reviewed production Gate deploy path anywhere. |

### Gaps, not defects

- **No frontend tests at all.** `encodeSend`, `encodeFinalize`, `encodeAutoParamsTo`, `encodeSwapIntent`, `extractSent`, `parseUnits` are pure and trivial to test, and a wrong byte offset misdirects funds silently. Highest-value test work in the repo.
- **No `eslint.config.mjs`.** Five `eslint-disable` comments currently suppress nothing.
- **No error boundary.** A throw in any view blanks the page.
- **Accessibility.** `Dropdown` is mouse-only, the detail drawer has no focus trap, Explorer rows are unreachable by keyboard.
- **Contract coverage.** No test walks `setPrice` across repeated calls (M5), and none passes an out-of-range `chainIdTo` (M1).
- **11 clippy warnings**, all cosmetic.

---

## Next

1. H1. Branch on `receipt.status()`.
2. H2. Add `indexer` and `graphql-api` to the Dockerfile and compose.
3. M8 and L13. Set a guardian. Write a deploy script with post-deploy assertions (checklist in `docs/operations.md` §5).
4. M2. Stop advancing cursors past failures, all three sites. Persist `paused`.
5. M4. `deny_unknown_fields` on the validator config, and make `block_confirmation = 0` an explicit opt-in on sources as it already is on refunds.
6. M1, M3, M7, M6, in that order.
7. L12. Unblock `forge coverage`, then act on what it says.
8. Frontend tests.

---

## Verification state

Run after the restructure, on this machine:

| Check | Result |
| --- | --- |
| `cargo build --workspace` | clean |
| `cargo test --workspace` | 40 passed, 0 failed. Same as the pre-move baseline. |
| `cargo clippy --workspace --all-targets` | 11 warnings, all cosmetic, none new |
| `bun install && bun run build` (frontend) | clean, 47 modules, 194 kB / 60.6 kB gzipped |
| `docker compose config` | OK |
| `git grep "bridge/"` | no hits outside `docs/history` |

Verified in the original review, not re-run since the move:

| Check | Result |
| --- | --- |
| `forge test` | 99 tests, 8 suites, 0 failures. Foundry 1.7.1, forge-std v1.9.4, OZ v5.0.2. |
| Solidity/Rust hash equivalence | Fixtures regenerated from Solidity byte-identical, Rust suites pass against them |
| All 9 frontend selectors | Keccak-256 recomputed, 9/9 match |
| Contract sizes | Largest is SwapRouter at 6,507 B, 18 KB under EIP-170 |
| `forge build --force` | 2 warnings, both on test helpers, none in `src/` |

**Not verified, and this is the real gap.**
Foundry is not installed on this machine, so no `scripts/testing/*.sh` has been run end to end since the restructure.
`bash scripts/testing/e2e.sh` is the check that actually proves `226a66f`.
Install Foundry and run it before trusting the scripts.

Two scripts also need external toolchains nobody here has: `build-solana.sh` needs Solana 1.18 platform-tools, and `solana-localnet-e2e.sh` needs a `solana-test-validator` container plus a built `.so`.
Their file dependencies are restored and their helper binary (`gen_claim_ix`) builds and emits the JSON shape `claim.mjs` expects, but neither script has been run.

---

## Loose ends

- `bridge/target/` is an orphaned 4 GB build cache. Delete it once you trust the new root.
- No production serving story for the frontend in compose. Dev relies on the Vite proxy; production needs same-origin serving or `VITE_API`.
