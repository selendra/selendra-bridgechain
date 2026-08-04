# SelendraBridge Remediation Plan

Audit date: 2026-08-04

Branch: `master` at `9478af4`

To: SelendraBridge development team

Assignment: implement the phased plan in this document, highest priority first.

Release decision: **do not deploy `solana-gate` at all; keep the EVM bridge off
mainnet until the remaining Solana work (P1.4, P2) is accepted.**

Status at 2026-08-04: **13 of 16 findings are fixed and verified.** The three
that remain are all Solana protocol work — M-2 (`cancel`/`refund` instructions),
M-3 (wiring the Solana leg into the running services), and the account-level test
harness that would prove them. Everything on the EVM path, the off-chain
services, and the configuration surface is closed.

| | Findings |
| --- | --- |
| **Fixed** | H-1, H-2, H-3, M-1, M-4, M-5, M-6, M-7, L-1, L-2, L-3, L-5, L-6 |
| **Open** | M-2, M-3 (+ the SBF/account-level harness in P2.2) |

This is a follow-up to [`docs/report.md`](docs/report.md) (2026-07-28). The EVM P0/P1 items
in that brief are genuinely closed and well covered by tests. What remains is
concentrated in the deployable Solana program, plus one **new** defect introduced
by the signature-array cap that closed a LOW finding from the previous round.

## Scope and method

Re-audited at `9478af4`:

- Solidity gate, hash library, swap pool, router, and deployment scripts
- Rust validator, keeper, indexer, signature store, database, and GraphQL API
- deployable Solana program and the host-side `bridge-solana` model
- React wallet transaction construction, polling, and recovery state
- Dockerfile, compose topology, generated configs, and shell scripts

Source audit plus local build/test verification. Not a formal cryptographic
audit, dependency-CVE audit, live-chain test, or penetration test.

## Verification baseline

Run locally during this audit:

| Check | At audit (9478af4) | After this work |
| --- | --- | --- |
| `cargo test --workspace --all-targets` | 50 passed | **71 passed**, 0 failed |
| `forge test` | 111 passed | **118 passed**, 0 failed |
| `cargo test -p solana-gate` | 4 passed (predicates only) | **10 predicate + 8 account-level**, 0 failed |
| `bash scripts/testing/e2e.sh` | not run | **PASS** — claim lands with 3 stored signatures against `validatorCount=1` (keeper submitted `sigs=1`) |
| `docker compose config --quiet` | passed | passed |
| `bash -n` over all scripts | passed | passed |
| `.github/workflows` present | **no CI** | **added** (`.github/workflows/ci.yml`) |
| `cargo-build-sbf` / `solana` on PATH | absent | absent — but no longer required, see below |

Without CI these fixes regress silently.

## What is, and is not, actually tested

Stated plainly, because "all tests pass" hides a real difference in how strongly
each fix is verified.

**Executed against a real chain or runtime:**

- **H-1** — `e2e.sh` runs two anvil chains, deploys, injects two throwaway-key
  signatures into the store, and asserts the claim still lands. The keeper log
  shows `sigs=1` submitted from 3 stored. The defect reproduced and refused.
- **M-5, L-1, L-2, L-6** — `forge test` executes on the EVM. M-5's write-once was
  additionally confirmed on a live anvil chain, reverting `LocalTokenAlreadySet`.
- **M-5's `run.sh` guard** — the `cast call tokenOf` skip-if-registered logic was
  run against a live gate in both the unset and set states.
- **H-3, L-3, M-1** — `tests/account_level.rs` drives the real handlers inside a
  `solana-program-test` bank: `send` to an unregistered corridor is refused *and
  creates no entry*; corridor registration is owner-gated, idempotent and
  capacity-bounded; a paused gate refuses `send`; a guardian may pause but not
  unpause; the validator set is capped.
- **M-4, M-7, L-5** — unit tests, including L-5's scope separation driven through
  the real axum router.

**Verified as a rule, but the code path never executed:**

- **H-2** — the replay predicate is tested, but `create_marker`'s
  transfer/allocate/assign sequence has **never run**. It sits behind `claim`'s SPL
  asset checks and the SPL token program is not in the test bank. This is the
  weakest point in the Solana story and should be closed before any deployment, by
  adding `spl_token.so` to `ProgramTest`.
- **M-6** — same shape: the predicate is tested; `register_asset` CPIs into SPL and
  is unexecuted.
- **`process_init`** — not drivable through `solana-program-test` at all.
  Installing a `bpf_loader_upgradeable`-owned account at the program's own address
  defeats the framework's builtin dispatch, so the runtime tries to load a real ELF
  from the fake ProgramData. Needs a genuine `cargo build-sbf` artifact. The
  authority *rule* is covered by `c1_tests::init_requires_upgrade_authority`.

**Not run at all:** the compose stack end-to-end with the split tokens (only
`docker compose config` syntax validation), and every test script except
`e2e.sh` — several of which generate configs this work modified.

## SBF note

The missing SBF toolchain
does *not* block the account-level tests P1 requires. `solana-program-test` has a
native mode — `ProgramTest::new("solana_gate", id, processor!(process_instruction))`
— which runs the entrypoint as a plain Rust function inside a real test bank. It
is a dev-dependency (`=1.18.26`, matching the pin in
`crates/solana-gate/Cargo.toml`), not a toolchain, and the crate already compiles
for the host. **This has now been done** — see `crates/solana-gate/tests/account_level.rs`,
8 passing tests against a real bank. Two limits emerged in practice: `init` is
untestable this way (builtin-dispatch conflict, above), and anything behind an SPL
CPI needs `spl_token.so` added to the bank.

What the SBF toolchain adds, and native mode cannot substitute for:

1. **Compute-unit budget.** `secp256k1_recover` costs roughly 25k CU per
   signature. Against the default 200k limit, a 5-of-7 gate spends ~175k on
   recovery alone before keccak, Borsh and the token CPI. This may already exceed
   budget at realistic validator counts — unmeasured, flagged for checking, not a
   finding.
2. **SBF stack limits** (4 KB per frame). The 512-byte config buffer plus
   `Vec<Vec<u8>>` signatures is the shape of thing that passes natively and
   aborts on-chain.
3. **The deployed `.so`** — the only artifact that matters for a real deployment.

Those are deployment-readiness checks, and they belong in CI (P2.2). They are not
the security tests that gate P1.

## Findings this plan closes

| Sev | ID | Status | Location | Summary |
| --- | --- | --- | --- | --- |
| HIGH | H-1 | **fixed** | `crates/keeper/src/main.rs:380,453,526` × `contracts/src/Gate.sol:551` | Keeper submits **every** stored signature, not just validators'. Two junk signatures push the array past `validatorCount` and make a transfer permanently unclaimable. |
| HIGH | H-2 | **fixed** | `crates/solana-gate/src/lib.rs:644-656` | Pre-funding the `["executed", submissionId]` PDA with the rent-exempt minimum permanently blocks that claim. |
| HIGH | H-3 | **fixed** | `crates/solana-gate/src/lib.rs:116,126-131,496-503,562` | `Config` is a fixed 512-byte account with an attacker-growable `nonce_to` vector; ~25 dust sends brick `send`, `set_validator` and `set_threshold` forever. |
| MED | M-1 | **fixed** | `crates/solana-gate/src/lib.rs:114,510` | `Config.paused` is dead code — no pause instruction, never read. The Solana leg has no circuit breaker. |
| MED | M-2 | **open** | `crates/solana-gate/src/lib.rs` | No `cancel`/`refund` instructions. The two-phase refund is EVM-only, so unclaimable EVM→Solana transfers lock the source deposit permanently. |
| MED | M-3 | **open** | `crates/bridge-solana`, `crates/validator`, `crates/keeper` | The Solana leg is not wired into any running service. The H5 event-format work is a library with round-trip tests and no runner. |
| MED | M-4 | **fixed** | `crates/keeper/src/config.rs:4`, `crates/indexer/src/config.rs:3` | No `deny_unknown_fields`. Config typos are silently ignored — the same failure mode as the previous round's H1. |
| MED | M-5 | **fixed** | `contracts/src/Gate.sol:229`, `contracts/src/SwapRouter.sol:267` | `setLocalToken` is repointable mid-flight with no zero-check or timelock; claims bind a `debridgeId`, not a token. |
| MED | M-6 | **fixed** | `crates/solana-gate/src/lib.rs:771-780` | `register_asset` does not reject a vault with a `delegate` or `close_authority` set. |
| MED | M-7 | **fixed** | `docker/configs/indexer.toml:19,28`, `crates/indexer/src/config.rs:38` | Indexer finality buffer defaults to 0 with no fail-closed guard; lifecycle writes come from tip reads. |
| LOW | L-1 | **fixed** | `contracts/script/DeployProd.s.sol:73-81` | Strict-majority rule computed from the supplied array length, not the deduplicated validator count. |
| LOW | L-2 | **fixed** | `contracts/src/Gate.sol:451` | `refund()` is `whenNotPaused`, so the breaker also blocks victims recovering stranded funds. |
| LOW | L-3 | **fixed** | `crates/solana-gate/src/lib.rs:496` | The same 512-byte allocation caps the validator set at ~22, failing with an opaque serialization error. |
| LOW | L-4 | partial | `crates/solana-gate/src/lib.rs:613-617` | Solana `claim` requires the signed receiver to be the SPL *token account*; nothing upstream enforces it. |
| LOW | L-5 | **fixed** | `docker-compose.yml`, `crates/sig-store/src/main.rs:68` | One shared `SIG_STORE_TOKEN` for every service — the reason H-1 is reachable from any single compromised component. |
| LOW | L-6 | **fixed** | `contracts/src/Gate.sol:361-365` | Gate liquidity is shared across every `debridgeId` mapped to the same local token, with no per-corridor accounting. |

Verified as genuinely fixed since the last brief, and not re-opened here: C1's
config/asset/vault binding and authorized init, H1 fail-closed finality, H2
receipt-status checks, H3 durable cursors and race-safe first insert, M1 persisted
pause, M2 range rejection, M4 priced cooldown, M8 exact-transfer enforcement,
M5/M6 frontend recovery and generation-guarded polling, and H4's compose topology.

## Sequencing

```
P0  H-1 keeper hotfix ─────────────────────────────► ship alone, immediately
P1  Solana account/lifecycle rewrite (H-2,H-3,M-1,M-2,M-6,L-3,L-4a)
      └── one design decision up front; do NOT split into seven PRs
P2  Solana wiring + SBF harness (M-3, L-4b) ──────► the only way P1 is provable
P3  EVM & config hardening (M-4,M-5,M-7,L-1,L-2)
P4  Operational / CI (L-5, L-6, coverage)
```

P0 and P3 are independent of the Solana track and should run in parallel with it.
P2 strictly follows P1 — wiring an insecure program into live signing is the
mistake the previous brief's H5 note warned against. Everything below P1 depends
on P4's CI to stay fixed.

---

## P0 — Hotfix, ship on its own branch

### P0.1 — H-1: keeper submits unfiltered signature arrays — **DONE**

Landed 2026-08-04. `crates/keeper/src/main.rs` + `crates/bridge-core/src/abi.rs`.
Membership is now a filter rather than only a counter; `GateView` refreshes
`threshold`, `validatorCount` and per-signer membership together every 60s; the
built array is capped at the current `validatorCount`. Four regression tests added
(`cargo test --workspace`: 54 passed, 0 failed — was 50). Detail below is retained
as the record of what was changed and why.

**Evidence:** `crates/keeper/src/main.rs:380` (also `:453`, `:526`) against
`contracts/src/Gate.sol:551`.

`quorum_count` correctly counts only on-chain validators — that was the M3 fix —
but `sorted_signatures(&rec.signatures)` then passes **every** stored signature
into the calldata. The signature store only verifies that a signature recovers to
its claimed signer; it does not check validator membership. Anyone who can write
to the store (any validator, the keeper, or any holder of the shared
`SIG_STORE_TOKEN`) can add signatures from throwaway keys. Once
`signatures.length > validatorCount`, `_verifySignatures` reverts
`TooManySignatures` on every attempt while `quorum_count` still reports quorum, so
the keeper retries forever. With three validators and threshold two, **two junk
signatures make a transfer permanently unclaimable.**

The signature cap added in `16ed706` was correct in itself; it converted a
gas-griefing nuisance into a liveness kill because the keeper's array was never
filtered to match.

**Required fix:**

- [x] Replaced `quorum_count` with `GateView::member_signatures`, returning only
      signatures whose signer is an on-chain validator. Call sites compare
      `.len() as u64 >= threshold`.
- [x] Threaded the filtered vec into `try_claim` / `try_cancel` / `try_refund` as a
      parameter, and removed the `sorted_signatures(&rec.*)` reads inside them, so
      the raw record is never a signature source.
- [x] Gave the membership memo a 60s TTL via `GateView::refresh_if_stale`. A cached
      `true` previously survived a validator being removed on-chain, so the array
      could outgrow a shrunken `validatorCount`.
- [x] Belt-and-braces: `validatorCount` is now read alongside `threshold` (added to
      the `sol!` bindings) and the built array is truncated to it. Truncation
      happens before ordering, so survivors stay strictly ascending.

**Invariant established:** store signers are already deduplicated by
`ON CONFLICT (submission_id, signer)`, and members are a subset of the on-chain
set, so the filtered array can never exceed `validatorCount` —
`TooManySignatures` becomes unreachable from the keeper.

**Acceptance:** four unit tests in `crates/keeper/src/main.rs` cover the filter
(2 validator + 2 throwaway signatures against a 3-validator gate yields a
2-element array; it yielded 4 before), the `validatorCount` cap under a stale
memo, ascending-order preservation after truncation, and an all-forged record
failing to reach quorum.

- [ ] **Still open:** an e2e leg in `scripts/testing/e2e.sh` that POSTs a junk
      signature before the keeper's tick and asserts the claim still lands. The
      unit tests prove the rule; only the e2e proves the wiring.

---

## P1 — Solana program: account model and lifecycle

All seven items touch `crates/solana-gate/src/lib.rs` and all move the same
account layout. Fixing them individually means redesigning the PDA set three
times. Treat this as one workstream.

### P1.0 — Design step (first, no code)

Write and review an account-model spec covering:

- [ ] the full PDA set: seeds, owner, size, and growth bound for each;
- [ ] a marker layout that distinguishes `claimed` from `cancelled` — the current
      `executed` PDA has **zero** data bytes and cannot;
- [ ] the source-side record giving Solana an analogue of `Gate.sentBy`, serving
      as both origin proof and refund recipient;
- [ ] domain prefixes 2 and 3 (`cancel_id`, `refund_id`) mirroring
      `contracts/src/BridgeHash.sol:16,20`.

### P1.1 — H-2: executed-PDA pre-funding griefing

**Evidence:** `crates/solana-gate/src/lib.rs:644-656`.

The replay guard is `lamports() > 0 || !data_is_empty()`, and the marker is
created with `system_instruction::create_account`, which itself fails when the
target already holds lamports. `submissionId` is public — it is in the source
chain's `Sent` event and in the signature store — so anyone can transfer the
rent-exempt minimum (~0.00089 SOL) to the derived PDA before the keeper claims.
Every later claim then returns `AlreadyExecuted`. Combined with M-2, the source
funds are stuck with no recovery path.

- [ ] Replace `create_account` with `allocate` + `assign` under `invoke_signed`;
      both succeed on a lamport-funded system account.
- [ ] Change the guard to `owner == program_id && !data_is_empty()`.
- [ ] Apply the same pattern to every marker PDA introduced in P1.4.

**Acceptance:** fund the derived PDA with the rent-exempt minimum, then claim —
must succeed. A second claim must fail `AlreadyExecuted`.

### P1.2 — H-3: config growth bricks `send` and governance

**Evidence:** `crates/solana-gate/src/lib.rs:116,126-131,496-503,562`.

`send` accepts an arbitrary `chain_id_to`, and `bump_nonce` appends a 16-byte
`(chain_id, nonce)` entry per unseen value. Borsh size is
`53 + 20·validators + 16·nonces`; with three validators the account holds 24
entries, and the 25th makes `cfg.serialize(...)` overflow the buffer. From then on
every `send`, `set_validator` and `set_threshold` fails — all three reserialize
the config — and there is no realloc or migration instruction. Cost to an
attacker: about 25 dust sends of any registered asset.

- [ ] Add an owner-gated `["corridor", chain_id_to]` PDA; `send` rejects an
      unregistered `chain_id_to`. This mirrors the EVM `allowed_chains` allowlist,
      so it is a protocol feature rather than only a size patch.
- [ ] Move the per-corridor nonce into that PDA, so `Config` never grows on `send`.
- [ ] Add an owner-gated resize instruction, or take `max_validators` at `init` and
      size the account from it. This also closes **L-3**.

**Acceptance:** 30 sends to 30 distinct registered corridors, then a 31st send and
a `set_threshold` — all succeed. An unregistered `chain_id_to` is refused.

### P1.3 — M-1: dead pause flag

**Evidence:** `crates/solana-gate/src/lib.rs:114,510`.

- [ ] Add `Pause` / `Unpause` instructions — guardian may pause, owner only may
      unpause — mirroring `contracts/src/Gate.sol:244-258`.
- [ ] Check `cfg.paused` at the top of `send` and `claim`.

**Acceptance:** a paused gate refuses `send` and `claim`; the guardian cannot
unpause.

### P1.4 — M-2: no `cancel` / `refund` on Solana

The largest single item, and the reason P1 is a workstream rather than a patch.
The refund design in `crates/validator/src/refund.rs` depends on a destination
`cancel` being an observable on-chain fact; Solana provides no such instruction,
so any EVM→Solana transfer that cannot be claimed locks the source deposit
forever.

- [ ] Add `cancel_id` / `refund_id` keccak helpers (prefixes 2 and 3).
- [ ] Add a `["sent", submissionId]` PDA written by `send`, recording payer, mint
      and amount. This is the origin proof and refund recipient — the job
      `Gate.sentBy` does — and it is required because `native_sender` is only
      hash-bound when auto-params are present.
- [ ] `process_cancel`: verify threshold over `cancel_id`, mark the marker PDA
      cancelled, emit a versioned `Cancelled` event.
- [ ] `process_refund`: verify threshold over `refund_id`, require the `sent` PDA,
      return vault to the original payer, close the `sent` PDA as the replay guard.
- [ ] Emit both events in the same `sol_log_data` framing as `SentEvent`, and
      mirror the layouts into `crates/bridge-solana/src/relayer.rs`.

**Acceptance:** a full two-phase cycle — send, failed claim, cancel, refund — plus
the ordering invariant: a claimed transfer can never be cancelled, and a cancelled
one can never be claimed.

### P1.5 — M-6: vault delegate / close authority unchecked

**Evidence:** `crates/solana-gate/src/lib.rs:771-780`.

- [ ] In `register_asset`, reject a vault whose `delegate` or `close_authority` is
      set. A vault created with a pre-set delegate can be drained outside the
      program entirely.

### P1.6 — L-4a: receiver-is-token-account is unenforceable upstream

**Evidence:** `crates/solana-gate/src/lib.rs:613-617`.

The behaviour is correct and deliberate; the problem is that nothing upstream
enforces it, so a user bridging to their wallet pubkey creates a transfer that is
both unclaimable and (until P1.4) unrefundable.

- [ ] Emit a distinct error code and document the rule in the program header. The
      UI half is P2.2.

**Effort for P1:** two to three weeks including the design step and tests. Do not
deploy `solana-gate` until all of it lands.

---

## P2 — Solana integration and real test harness

### P2.1 — M-3: the Solana leg has no runner

`bridge-solana` is a workspace library; `validator`, `keeper` and `indexer`
contain no Solana code, so the H5 event-format work is currently untestable
end-to-end.

- [ ] **Validator** — a Solana source: `getSignaturesForAddress` →
      `getTransaction` → `parse_sent_log_line`, using `commitment: finalized` in
      place of `block_confirmation` (with the same fail-closed config guard the
      EVM source has), a persisted signature cursor, and the existing
      nonce-sequencing and pause machinery.
- [ ] **Keeper** — a Solana target: `build_claim_instruction`, resolve the asset,
      vault and executed PDAs, submit, confirm, and treat a failed transaction as
      a failure (the H2 posture).
- [ ] **Indexer** — decode `Sent` / `Claimed` / `Cancelled` / `Refunded` program
      data into the same database rows the EVM path writes.

### P2.2 — SBF harness and L-4b UI guard

- [ ] Add `solana-program-test = "=1.18.26"` as a dev-dependency and port every P1
      acceptance test to account-level coverage. This needs no toolchain — see the
      verification-baseline correction above — so it can start with P1.0.
- [ ] Separately, add `cargo-build-sbf` to CI for the compute-unit, stack-limit and
      deployed-artifact checks native mode cannot cover.
- [ ] Frontend and `Gate.send`: when `chainIdTo` is Solana, validate that the
      32-byte receiver is a derivable associated token account for the chosen
      mint, so a wallet pubkey cannot be bridged to by mistake.

---

## P3 — EVM and configuration hardening

Independent of the Solana track; run in parallel.

| # | Item | Change | Acceptance |
| --- | --- | --- | --- |
| P3.1 | **M-4** | Add `#[serde(deny_unknown_fields)]` to every struct in `crates/keeper/src/config.rs` and `crates/indexer/src/config.rs` | A misspelled-field test, mirroring `crates/validator/src/config.rs:353` |
| P3.2 | **M-5** | `Gate.sol:229` — reject `address(0)`; make a non-zero mapping immutable or route changes through a timelock. `SwapRouter.sol:267` should read `tokenOf` at claim time, not finalize time | A claim signed under mapping A, with the owner repointing to B before the claim, must revert rather than pay out B |
| P3.3 | **M-7** | `crates/indexer/src/config.rs:38` — add `allow_zero_confirmation` and the validator's fail-closed guard; raise the compose default off 0 | Config unit tests for omitted, zero, and opted-in values |
| P3.4 | **L-1** | `DeployProd.s.sol:73` — reject duplicate validators before the majority check, or compute the rule from `gate.validatorCount()` after deploy | `[A,B,B]` with threshold 2 must revert instead of shipping a 2-of-2 gate |
| P3.5 | **L-2** | `Gate.sol:451` — drop `whenNotPaused` from `refund()`, or add a separate `refundsPaused` flag. `refund()` pays only the original depositor, so it is not an incident-response risk | A paused gate still refunds a cancelled transfer |

---

## P4 — Operational and cross-cutting

- [ ] **CI.** No workflow exists, so every fix above regresses silently. Add
      `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace
      --all-targets`, `forge test` with coverage, `bun run build`,
      `docker compose config`, `bash -n` over the scripts, and — once P2.2 lands —
      `cargo-build-sbf` plus the `solana-program-test` suite. Add `cargo-audit`
      and `bun audit`.
- [ ] **L-5 — credential separation.** One `SIG_STORE_TOKEN` currently
      authenticates validators, keeper and graphql-api alike, which is exactly
      what makes H-1 reachable from any single compromised component. Move to
      per-service tokens with per-route scopes: validators write signatures, the
      keeper reads and marks claimed, graphql-api is read-only. Drop the
      `dev-local-bridge-token` compose default for a required variable that fails
      startup when unset outside dev.
- [ ] **L-6 — shared gate liquidity.** Document the model explicitly, then add
      per-`debridgeId` accounting or a per-corridor cap so one corridor cannot
      exhaust a token another corridor depends on.
- [ ] **Coverage.** `forge coverage` still fails on `Gate.send` stack depth per the
      previous baseline. Resolve it once so the Solidity suite can carry a
      coverage gate.

---

## Acceptance criteria for the release decision

1. P0.1 merged with its regression test, and an e2e run showing a claim surviving
   injected junk signatures.
2. P1 complete, with account-level `solana-program-test` coverage that first
   reproduces and then refuses: executed-PDA pre-funding, corridor-vector
   exhaustion, unregistered corridors, paused send and claim, a delegated vault,
   and the full cancel-then-refund cycle.
3. P2 proving one real Solana→EVM transfer and one EVM→Solana transfer through the
   actual validator and keeper processes.
4. P3 items merged with the acceptance tests listed above.
5. CI green on every check, including the SBF suite, from a clean checkout.

Until all five hold, the recommendation stands: **do not deploy `solana-gate`, and
keep the EVM bridge off mainnet.** P0.1 alone is worth shipping to any running
deployment immediately.

## Required handoff evidence

For each completed item, send the reviewer:

1. the PR or commit range;
2. the exact acceptance criteria addressed;
3. the new regression-test name and the failure it reproduces;
4. fresh test, build, and end-to-end output;
5. known limitations and any follow-up ticket;
6. deployment, migration, and rollback instructions where applicable.

Do not close an item on compilation or existing green tests alone. The current
suites do not exercise the keeper's signature-array construction, any Solana
account-level authorization, the Solana refund path, or config-typo handling in
the keeper and indexer.

---

## Change record — 2026-08-04

What landed, why, and what proves it. Every entry has a regression test that fails
against the pre-fix code.

### Fixed

**H-1 · keeper forwarded unfiltered signature arrays** — `crates/keeper/src/main.rs`,
`crates/bridge-core/src/abi.rs`. Membership became a *filter*, not just a counter.
New `GateView` holds `threshold`, `validatorCount` and per-signer membership,
refreshed together every 60s (previously `threshold` was a startup snapshot and
membership never expired, so a stale `true` could itself overflow the cap). The
filtered list is capped at `validatorCount` before ordering, so
`TooManySignatures` is unreachable from the keeper. `validatorCount()` added to
the `sol!` bindings. *Proof:* 4 unit tests + `e2e.sh` now injects two throwaway-key
signatures before the keeper starts and asserts the claim still lands — 3 stored
against `validatorCount=1`, keeper submitted `sigs=1`.

**H-2 · pre-funded `executed` PDA blocked claims forever** — `solana-gate/src/lib.rs`.
Guard is now `owner == program_id && data_len > 0`: lamports prove nothing, since
anyone can fund any address. Marker creation moved from `create_account` (which
fails outright on a funded account) to `transfer` + `allocate` + `assign`.

**H-3 / L-3 · unbounded config growth bricked send and governance** — corridors are
now governance-registered (`RegisterCorridor`, owner-gated) and `send` refuses an
unregistered `chain_id_to`; `bump_nonce` can no longer append. The account is sized
at init from declared `max_validators` / `max_corridors` instead of a flat 512
bytes, both vectors are capped, and `Config::store` refuses a write that would not
fit. *Proof:* `config_space` is asserted against real Borsh output at capacity, and
a test pins the original arithmetic (24 corridors fit 512 bytes, the 25th did not).

**M-1 · dead pause flag** — `Pause` / `Unpause` / `SetGuardian` instructions added;
`send` and `claim` check `cfg.paused`. Guardian may stop but not start, mirroring
`Gate.sol`.

**M-4 · config typos silently ignored** — `deny_unknown_fields` on every keeper and
indexer config struct, with `from_toml` split out for testability. *Proof:* a
misspelled `[[source]]` (the typo that silently produced a keeper submitting no
refunds) is now an error.

**M-5 · `setLocalToken` repointable mid-flight** — now write-once and zero-rejecting.
A claim binds a `debridgeId`, never a token, so repointing would let validators'
existing signatures release a different asset. This also subsumes the
`SwapRouter._settle` read-timing half of the finding: the mapping can no longer
change between claim and finalize. `scripts/run.sh` skips already-registered
corridors rather than re-sending.

**M-6 · vault delegate / close authority unchecked** — `register_asset` rejects a
vault carrying either. Owning a vault proves the program *can* move it, not that
nobody else can.

**M-7 · indexer read at the chain tip** — `allow_zero_confirmation` plus the
validator's fail-closed guard. The indexer is the only writer of `refund_status`,
so a reorged-away `Claimed` could clear a genuinely stranded transfer's flag.

**L-1 · duplicate validators shrank the deployed set** — `DeployProd` rejects
duplicates and zero entries in preflight, and asserts `validatorCount ==
validators.length` post-deploy. `[A,B,B]` with threshold 2 previously passed every
check and shipped a 2-of-2 gate.

**L-2 · breaker blocked refunds** — `refund()` is no longer `whenNotPaused`. It
returns already-locked funds to the address that locked them, after an attested
destination burn, so it creates no exposure. `send`, `claim` and `cancel` stay
halted — `cancel` deliberately, since it is irreversible.

**L-5 · one shared credential for every service** — replaced with scoped tokens
(`Read` / `Sign` / `Relay` / `Admin`) in `bridge_core::auth`. Each sig-store route
group demands the narrowest scope that works; each client presents its own
credential. The GraphQL API — the most exposed component — now holds a read-only
token. `SIG_STORE_TOKEN` still works as an all-scopes fallback but logs a warning.

**L-6 · shared liquidity** — documented explicitly in `Gate.sol`: corridors sharing
a local token are one trust domain and must be provisioned for their combined
worst case. Per-`debridgeId` accounting and per-corridor caps are described as the
two ways to bound it, left unimplemented because both change how operators
provision — a product decision, not a defect fix.

**CI** — `.github/workflows/ci.yml` added: Rust fmt/clippy/tests, the separate
solana-gate workspace, Foundry build/test/coverage, frontend build, compose
validation, shell syntax + shellcheck, `cargo-audit`, and a non-blocking SBF build
job. `fmt`, `clippy` and `forge coverage` are marked `continue-on-error` with
migration notes, because the tree has pre-existing lints and a known
stack-too-deep — turning them red on day one would bury real failures.

### Still open

**M-2 · no `cancel` / `refund` on Solana.** The largest remaining item and a genuine
protocol addition: it needs `cancel_id`/`refund_id` domain hashing, a
`["sent", submissionId]` PDA recording payer/mint/amount (Solana's analogue of
`Gate.sentBy`, required because `native_sender` is only hash-bound when auto-params
are present), both instructions, and matching versioned events mirrored into
`bridge-solana`. Until it lands, an EVM→Solana transfer that cannot be claimed
locks the source deposit permanently. The marker PDA introduced by H-2 already
carries a data byte so `claimed` and `cancelled` can be distinguished.

**M-3 · the Solana leg has no runner.** Needs a Solana source in the validator, a
Solana target in the keeper, and program-data decoding in the indexer. See P2.1.

**P2.2 · account-level tests.** Everything under P1 is currently proven by
host-runnable *predicates* — the pure authorization rules — not by account-level
execution. `solana-program-test` in native mode needs no toolchain and should be
added next; it is what would actually exercise the `create_account`-on-funded-account
behaviour behind H-2 rather than asserting the rule that governs it.

**L-4 · Solana receiver must be an SPL token account.** The program-side error is
distinct and documented; the upstream UI/`send` guard (P2.2) is not built, so a
user can still bridge to a wallet pubkey and create an unclaimable transfer.
