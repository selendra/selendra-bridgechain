# SelendraBridge Remediation Brief

Audit date: 2026-07-28

Branch: `master` at `e8fb32a`

To: SelendraBridge development team

Assignment: implement and verify the remediation plan in this document.

Release decision: **do not deploy to production or public testnet with real
funds until every P0 item is accepted.**

## What the development team must deliver

Please implement the P0 work in the specified dependency order. Submit the work
as reviewable PRs rather than one large patch. Each PR must include:

- a regression test that fails before the fix and passes afterward;
- the relevant unit, integration, and end-to-end verification output;
- configuration and operations documentation updates;
- migration or rollout instructions when persisted state, database schema, or
  deployment topology changes;
- a short statement identifying which acceptance criteria below are satisfied.

Do not close an item based only on compilation or existing green tests. The
current suites do not exercise the critical Solana authorization boundary,
chain reorgs, cursor fault recovery, concurrent first writes, or the actual
compose refund topology.

## Executive status

The EVM hash/signature design has strong unit coverage, and the checked build
baseline is green. The repository still has one critical authorization defect,
five high-severity safety/liveness defects, and no CI gate.

The most important correction to the previous report is the Solana program.
Its hashing and signature primitives are correct, but its `claim` instruction
does not bind those checks to the canonical config or an asset-specific vault.
That makes the deployable program unsafe even though the host-side Solana model
passes.

Do not deploy `solana-gate`. Keep the EVM bridge restricted to local development
until the P0 list below is closed and the end-to-end suites have been rerun.

## Scope and method

Reviewed:

- Solidity bridge, refund, swap pool, router, deploy scripts, and Foundry tests
- Rust validator, keeper, indexer, signature store, database, GraphQL API, and
  host-side Solana model
- deployable Solana program and local-validator driver
- React wallet transaction construction, cross-chain flow state, API polling,
  and production build
- Docker image/compose wiring, shell scripts, configuration, documentation, and
  repository test/CI surface

This was a source audit plus local build/test verification. It was not a formal
cryptographic audit, dependency-CVE audit, live-chain test, or penetration test.

## Findings

### CRITICAL C1 — Solana `claim` can use an attacker-controlled validator set

**Evidence:** `crates/solana-gate/src/lib.rs:359-430`.

`process_claim` deserializes whichever `config_ai` the caller supplies. It never
requires the canonical `["config"]` PDA or checks that the account is owned by
the program. The same path has no `debridge_id -> mint/vault` registry and does
not bind `vault` to the signed asset. It derives the global
`["vault_authority"]` PDA but does not otherwise establish which SPL vault that
authority may release.

An attacker can supply config bytes containing their own validators and
threshold, sign an invented claim, select an SPL token account controlled by the
program's global vault-authority PDA, and direct its balance to the signed
receiver. The SPL token program enforces mint equality and the PDA signature; it
does not repair the missing bridge authorization or asset binding.

The same canonical-config validation is absent from `send`, `set_validator`, and
`set_threshold`. `emit_sent` also emits only the id and debridge id, not the
fields a validator needs to reconstruct the event.

**Required fix:** stop deployment; require the canonical program-owned config
PDA on every instruction; add a program-owned asset/vault registry keyed by
mint/debridge id; verify token program, vault, vault authority, receiver mint,
and all writable owners. Make deployment and initialization atomic: today
`process_init` has no intended-deployer authority, so the first caller to create
the config PDA becomes owner. Replace the stale module documentation that says
the authority is `["vault", mint]`; the code currently uses one global
`["vault_authority"]`, which expands the blast radius to every vault.

Add `solana-program-test` tests that first reproduce config takeover and the
forged-config vault drain, then prove both fail. Repair the event/relayer
incompatibility in H5 only after the `send` config boundary is secure.

### HIGH H1 — source-chain finality defaults to zero and the apparent opt-in is discarded

**Evidence:** `crates/validator/src/config.rs:93-115,177-260` and
`docker/configs/val1.toml:8-9` (same shape in val2/val3).

`SourceChain.block_confirmation` defaults to zero. Unlike the refund reader,
startup does not reject zero confirmations. `SourceChain` has no
`allow_zero_confirmation` field, while serde accepts unknown fields, so the
shipped config's apparent explicit opt-in is silently ignored.

On a reorg-capable source chain, validators can sign a `Sent` event at the tip,
the keeper can release destination liquidity, and the source deposit can then
disappear in a reorg.

**Required fix:** add a real source-level opt-in intended only for local
instant-finality chains, reject zero by default, and use
`#[serde(deny_unknown_fields)]` on every operational config structure. Add
config tests for omitted, misspelled, zero, and explicitly opted-in values.

### HIGH H2 — a reverted EVM claim is persisted as claimed

**Evidence:** `crates/keeper/src/main.rs:393-405`,
`crates/keeper/src/main.rs:240-249`, and
`crates/bridge-db/src/lib.rs:489-503,770-790`.

`try_claim` logs the receipt status but returns success for status `0`. Its caller
then calls the signature store's `mark_claimed` route. Refund eligibility and
candidate queries exclude rows whose status is `claimed`, so a mined revert can
permanently hide a stranded transfer from recovery. `mark_claimed` also changes
an existing `refund_status = 'eligible'` back to `'none'`, actively undoing a
recovery flag the indexer sweep may already have raised.

`try_cancel` and `try_refund` also log success without checking receipt status.
They do not directly advance the database lifecycle, but they produce false
operator signals and retry only on a later poll.

**Required fix:** treat a receipt with failed status as an error in all three
paths. Prefer making the indexer the only writer of claimed/cancelled/refunded
lifecycle state, based on observed on-chain events. Do not remove the keeper
write until H4's indexer is actually built and running.

### HIGH H3 — failed logs are skipped while cursors advance

**Evidence:** `crates/indexer/src/main.rs:136-160,167-191` and
`crates/validator/src/main.rs:224-244`.

The indexer logs and suppresses both scan-level and per-log handler errors, then
persists the end of the block range. The validator logs a `handle_log` error and
also advances the range cursor. A transient database/store failure can therefore
drop history or a validator signature permanently.

This interacts badly with concurrent first writes to the signature database:
`bridge-db` performs `SELECT` then plain `INSERT` for a new submission, so
simultaneous validators can race on the primary key. A losing validator reports
an error and the current scan loop never revisits that event.

**Required fix:** a batch succeeds only when every relevant log is durably
handled. Return errors instead of swallowing them, advance the cursor only after
success, make first-write upserts race-safe, and add fault-injection/restart
tests.

### HIGH H4 — the shipped compose stack cannot run the advertised refund path

**Evidence:** `Dockerfile:7,14-16` and `docker-compose.yml:14-99`.

The image builds only validator, keeper, and signature store. Compose does not
run the indexer, which is the only component that marks transfers refund
eligible and records on-chain cancellation/refund state. The three validators
therefore poll a candidate list that never becomes populated through the
advertised stack.

GraphQL and the frontend are also absent, so compose is not a complete runnable
product surface. Containers run as root, have no restart policy, and expose
Postgres and the signature store to the host with development credentials/token.

**Required fix:** add indexer and GraphQL binaries/services, a frontend serving
story, non-root runtime user, health-based dependencies, restart policies, and
production-safe port/secret profiles. Prove claim and refund flows against the
actual compose topology.

### HIGH H5 — Solana `Sent` output and the relayer use incompatible protocols

**Evidence:** `crates/solana-gate/src/lib.rs:482-492` and
`crates/bridge-solana/src/relayer.rs:119-135`.

The program emits binary program data with two fields:
`sol_log_data([b"BRIDGE_SENT", id || debridge_id])`. The off-chain relayer does
not decode that format. It looks for a text line shaped as
`BRIDGE_SENT {json}` and expects the complete transfer record.

Solana-to-EVM scanning is therefore non-functional today. The missing data is
also why a validator could not independently reconstruct the submission id from
the deployed program's event.

**Required fix:** after C1's `send` config/account boundary is secured, define
one versioned event wire format shared by the program and relayer, emit every
hash-bound field plus the locked asset identity, and test the real transaction
log decoder. Fixing the parser first would expose the insecure `send` path to
live bridge signing.

### MEDIUM findings

| ID | Evidence | Risk and next change |
| --- | --- | --- |
| M1 | `crates/validator/src/state.rs:16-23,61-80,117-125` | Pause state and reason are not serialized. Restarting after a nonce anomaly or hash mismatch silently unpauses the validator. Persist the safety stop and require an explicit operator resume. |
| M2 | `crates/validator/src/main.rs:264-266,399-401`; `crates/indexer/src/main.rs:201-203` | `uint256` chain ids/nonces saturate to `u64::MAX`; database casts then use signed `BIGINT`. Reject out-of-range values on-chain and off-chain instead of aliasing them. |
| M3 | `crates/bridge-core/src/store.rs:229-253`; `crates/keeper/src/main.rs:204,222` | The store authenticates signatures but does not know validator membership, while keeper prechecks count every distinct signer. Outsider signatures can trigger repeated reverting submissions. Pin validator sets per destination or verify membership before quorum counting. |
| M4 | `contracts/src/SwapPool.sol:220-237` | The oracle's price-deviation cap is per call with no time/cumulative bound. A compromised oracle can walk the price arbitrarily in one block. Add a delay, epoch/cumulative bound, or independent oracle guard. |
| M5 | `frontend/src/components/BridgeView.tsx:267-360` | Cross-chain finalize state exists only in React memory. A refresh/device change loses the recovery action. The event parser also searches the user-editable gate address even though the router's immutable gate is authoritative. Rebuild pending state from indexed history and use registry/on-chain addresses. |
| M6 | `frontend/src/api/hooks.ts:26-59` | `usePoll.run` ignores effect generation and permits overlapping or superseded requests to overwrite current data. Add an abort/generation guard and avoid overlapping intervals. |
| M7 | `contracts/src/Gate.sol:227-246`; `contracts/script/DeployXSwap.s.sol:52-54` | No repository deployment path appoints a guardian or enforces production validator/chain parameters. Demo scripts deploy threshold-one gates and unrestricted-mint assets without a chain-id guard. Create a production-only deployment script with post-deploy assertions. |
| M8 | `contracts/src/Gate.sol:268-312,323-350` | Gate accounting assumes exact-transfer ERC-20s. Fee-on-transfer/rebasing assets can lock or release less than the signed amount and consume shared liquidity. Explicitly reject unsupported asset behavior or account by verified balance deltas and document the policy. |

### LOW findings and quality gaps

- `frontend/src/components/Dropdown.tsx:20-82` falls back visually to the first
  option without updating parent state and implements only partial listbox
  keyboard behavior.
- `frontend/src/components/BridgeView.tsx:124-150` retains old decimals while a
  new token read is pending; a fast submit can encode the wrong base amount.
- `frontend/src/wallet/eth.ts:254-331` does not include/check a transaction chain
  id immediately before wallet writes.
- `frontend/src/wallet/eth.ts:363-386` does not match the `Sent` topic and trusts
  fixed data offsets after matching only an address.
- `crates/graphql-api/src/swap.rs:85-118` performs two genesis-to-tip log scans
  whenever pool metadata is requested. Cache/index token-list state.
- `crates/graphql-api/src/swap.rs` returns zero for `max_swap_usd` on intermediate
  overflow where the contract's full-precision `mulDiv` can return a value.
- `contracts/src/Gate.sol:524-542` does not cap signature-array length. This is
  caller-paid grief rather than a vault compromise, but a sensible cap reduces
  pathological RPC/estimation load.
- `forge coverage` cannot compile `Gate.send` because of stack depth; the
  `--ir-minimum` fallback fails with a Yul stack exception too.
- There are no frontend tests, no database integration tests in the Rust test
  suite, no deployable Solana program tests, no dependency vulnerability scan,
  and no CI workflow.

## Required implementation plan

### P0 — release blockers

- [ ] **P0.1 — Solana authorization and initialization.** Reproduce C1 in
  `solana-program-test`, make deploy+init atomic/authorized, and enforce canonical
  config/asset/vault/token accounts. Acceptance: config front-running, forged
  config, wrong config owner/PDA, wrong mint, wrong vault, wrong authority, wrong
  token program, and replay all fail; a valid 2-of-3 claim passes.
- [ ] **P0.2 — Source finality.** Fail closed on zero confirmations unless a real
  source-level local-dev opt-in is present; reject unknown config keys.
  Acceptance: config unit tests cover omissions and typos, and a reorg simulation
  proves unconfirmed sends are never signed.
- [ ] **P0.3 — Runnable topology.** Ship indexer, GraphQL, and frontend alongside
  the existing services with non-root/restart/secret hardening. Acceptance: a
  clean compose run proves one claim and one timed-out cancel-then-refund, with
  history visible through GraphQL/UI.
- [ ] **P0.4 — Receipt truth; start only after P0.3.** Reject failed receipts, then remove
  keeper-authoritative claim lifecycle writes. Acceptance: a forced mined revert
  remains retryable/refund-eligible; only an indexed `Claimed` event marks
  success.
- [ ] **P0.5 — Durable cursors.** Fail the whole batch on any handler/store/database
  error and make concurrent database insertion idempotent.
  Acceptance: injected failures followed by restart produce every expected row
  and signature exactly once.
- [ ] **P0.6 — Solana event protocol; start only after P0.1.** Implement one shared versioned
  event encoder/decoder. Acceptance: a real program transaction is decoded into
  every hash-bound field, independently recomputed, signed, and delivered to the
  EVM test gate.

### P1 — safety and recovery

- [ ] Persist validator pause state and add restart tests.
- [ ] Reject chain ids/nonces outside the off-chain and database domains.
- [ ] Count only configured validators toward off-chain quorum readiness.
- [ ] Make cross-chain finalize recoverable after refresh and validate the actual
  router/gate pair.
- [ ] Add the production deployment script, guardian setup, multisig ownership,
  chain-id guards, and post-deploy assertions.
- [ ] Define and enforce the supported ERC-20 behavior policy.

### P2 — engineering quality

- [ ] Add frontend unit tests for every calldata encoder/parser and integration
  tests for wallet network changes, stale requests, refresh recovery, and failed
  receipts.
- [ ] Add database concurrency/lifecycle tests and sig-store API limits.
- [ ] Unblock Solidity coverage and add tests for cumulative price movement and
  out-of-range chain ids.
- [ ] Add CI for formatting, clippy, Rust tests, Foundry tests/coverage, frontend
  tests/build, compose validation, shell syntax, and the Solana program suite.
- [ ] Add pinned dependency vulnerability scanning for Cargo and Bun.
- [ ] Address accessibility, error-boundary, and GraphQL caching/pagination gaps.

## Required handoff evidence

For each completed item, send the reviewer:

1. the PR or commit range;
2. the exact acceptance criteria addressed;
3. the new regression-test name and the failure it reproduces;
4. fresh test/build/end-to-end output;
5. known limitations and any follow-up ticket;
6. deployment, migration, and rollback instructions where applicable.

The development team should not mark the bridge production-ready. Return the
completed evidence for a second security review and release decision.

## Verification baseline

Run locally during this audit:

| Check | Result |
| --- | --- |
| `cargo test --workspace --all-targets` | 40 passed, 0 failed |
| `forge test -vv` | 99 passed, 0 failed |
| `bun run build` | passed; 47 modules, 194.00 kB JS / 60.61 kB gzip |
| `cargo test --manifest-path crates/solana-gate/Cargo.toml --lib` | compiled; **0 tests**; 4 cfg warnings |
| `cargo clippy --workspace --all-targets` | exit 0 with 11 non-blocking warnings |
| `docker compose config --quiet` | passed |
| `bash -n docker/deploy.sh scripts/testing/*.sh` | passed |
| `forge coverage --report summary` | failed: stack too deep at `Gate.sol:308` |
| `forge coverage --ir-minimum --report summary` | failed: Yul stack exception |

Not run:

- live Postgres/database integration scripts
- Docker end-to-end flows
- EVM multi-process end-to-end scripts
- Solana SBF build and local-validator flow
- dependency-CVE scans

Passing unit/build checks do not cover C1, reorg behavior, cursor fault recovery,
database write races, or the shipped compose refund topology.

## Independent review

Claude Code 2.1.220 independently read the report and referenced source. Its
verdict:

- confirmed C1 and H1-H4 with no severity downgrade;
- identified H5 as a separate release blocker;
- confirmed that C1 is a config/asset authorization failure, not receiver
  redirection (the receiver itself is signature-bound);
- required the H2 eligibility-reset detail, atomic Solana initialization, and
  the P0.3-before-P0.4 dependency now recorded above;
- judged the report safe to hand to the team after those corrections.

Claude could not rerun the build/test commands in its sandbox, so the verification
table remains backed by the command output from this audit rather than by a
second execution.
