# Building a Cross-Chain Bridge From the Ground Up — Step-by-Step Plan

> **Historical.** This is the plan the bridge was built from, kept as a record of
> intent. It is not a description of the system as it stands - for that, read
> [`../architecture.md`](../architecture.md), which is written from the sources.
> Phases 0-8 are largely realised; where this document and the code disagree, the
> code wins.

> A pragmatic, **incremental** plan to build an external-validator bridge.
> Every phase ends with a **✅ Verification Checkpoint** — a concrete test that
> proves the step works before you move on. Do not skip checkpoints.

---

## Guiding principles

1. **Build the smallest thing that can be tested, then test it.** Each phase
   produces something runnable and verifiable on its own.
2. **Start local, single-machine.** Two local EVM chains on your laptop. No real
   testnets, no real money, no Docker until Phase 7.
3. **EVM-only first.** Add other ecosystems (Solana, etc.) only after the EVM
   happy path works end-to-end. This mirrors how this repo treats Solana as a
   pluggable add-on.
4. **The hash is sacred.** The `submissionId` hashing must be **byte-for-byte
   identical** in the contract and the validator. We test this equivalence
   explicitly and early (Phase 3) because it's the #1 source of "nothing works."
5. **One validator before many.** Get threshold=1 working end-to-end, then scale
   to N validators + threshold.

### Default tech stack (swap if you prefer)
| Layer | Choice | Why |
|-------|--------|-----|
| Contracts | **Foundry** (`forge`/`anvil`/`cast`) | fast tests, trivial local multi-chain via two `anvil` instances |
| Local chains | **2× `anvil`** (chainId 1337 & 1338) | instant, deterministic, free |
| Validator node | **This NestJS repo, adapted** | scanning/nonce/RPC-failover already solved |
| Signature store | **Local REST API first, Arweave later** | remove external deps while building |
| Keeper | **Small TypeScript script** | shares ethers/web3 + ABIs with the node |

> Prefer Hardhat? The structure is identical; replace `forge test` with
> `npx hardhat test` and `anvil` with `npx hardhat node`.

---

## Phase map (what we build, in order)

```
Phase 0  Workspace & two local chains running
Phase 1  Gate contract: send() locks + emits Sent  (one chain)
Phase 2  Gate contract: claim() verifies sigs + executes (replay-safe)
Phase 3  submissionId hashing — prove contract == off-chain (THE critical test)
Phase 4  Minimal validator: scan → recompute → sign → store (1 validator)
Phase 5  Keeper: collect sig(s) → submit claim() — FIRST END-TO-END TRANSFER
Phase 6  Harden the validator (adapt this repo: nonce, finality, multi-RPC, DB)
Phase 7  Scale to N validators + threshold; dockerize
Phase 8  Asset registry + wrapped-token minting (bridging new assets)
Phase 9  Testnet soak + chaos + audit prep
```

---

## Phase 0 — Workspace and two local chains

**Goal:** a repo skeleton and two independent EVM chains you can talk to.

**Build:**
```
bridge/
├── contracts/        # Foundry project (forge init)
├── validator/        # adapted copy of this NestJS node (later phases)
├── keeper/           # TS script (later)
└── sig-store/        # tiny REST API (later)
```
```bash
mkdir bridge && cd bridge
forge init contracts
# Terminal A — "source" chain
anvil --chain-id 1337 --port 8545
# Terminal B — "target" chain
anvil --chain-id 1338 --port 8546
```

**✅ Verification Checkpoint 0:**
```bash
cast chain-id --rpc-url http://127.0.0.1:8545   # => 1337
cast chain-id --rpc-url http://127.0.0.1:8546   # => 1338
cast block-number --rpc-url http://127.0.0.1:8545
```
Both return values and block numbers increment. You now have a "source" and a
"target" chain.

---

## Phase 1 — Gate contract: `send()` (lock + emit)

**Goal:** lock an ERC-20 on the source chain and emit a `Sent` event carrying all
the parameters a validator needs.

**Build (`contracts/src/Gate.sol`):**
- A per-target-chain **nonce** counter: `mapping(uint256 chainIdTo => uint256) nonce`.
- `send(address token, uint256 amount, uint256 chainIdTo, bytes receiver, bytes autoParams)`:
  1. `transferFrom` the token into the gate (lock),
  2. compute `submissionId` (a placeholder hash for now — finalized in Phase 3),
  3. `emit Sent(submissionId, debridgeId, amount, chainIdFrom, chainIdTo, receiver, nonce, autoParams)`,
  4. increment nonce.
- Keep a `getChainId()` view (the validator's `Web3Service` checks this against
  config — see `Web3Service.validateChainId`).

**Test (`contracts/test/Send.t.sol`):**
- mint test token → approve → `send()`;
- assert the `Sent` event fires with expected fields;
- assert tokens are now held by the gate;
- assert nonce incremented; two sends to the same target give nonces `n, n+1`.

**✅ Verification Checkpoint 1:**
```bash
cd contracts && forge test --match-contract Send -vvv
```
All `Send` tests green. You can lock funds and emit a well-formed event.

---

## Phase 2 — Gate contract: `claim()` (verify + execute, replay-safe)

**Goal:** on the target chain, verify validator signatures and release funds
exactly once.

**Build (extend `Gate.sol`):**
- Validator set + threshold: `mapping(address => bool) isValidator; uint256 threshold;`
  (governance-settable; for now set in constructor).
- `mapping(bytes32 submissionId => bool) executed;` — **replay guard**.
- `claim(<all submission params>, bytes[] signatures)`:
  1. **recompute** `submissionId` from params (same hash as `send`),
  2. `require(!executed[submissionId])`,
  3. recover each signer (`ecrecover` over the EIP-191/`toEthSignedMessageHash`
     of `submissionId`), require each is a validator and **distinct**, count them,
  4. `require(count >= threshold)`,
  5. mark `executed`, then mint/unlock to `receiver` and (optionally) execute
     `autoParams.data`.

> Match the signing scheme to the node: this repo signs with
> `account.sign(submissionId)` (web3 `eth_sign` over the message), so the
> contract must verify with the matching `toEthSignedMessageHash` prefix.

**Test (`contracts/test/Claim.t.sol`):**
- happy path: 1 valid sig, threshold 1 → receiver paid, `executed` set;
- **replay**: second identical `claim` reverts;
- **insufficient sigs**: threshold 2, give 1 → revert;
- **bad signer**: sign with a non-validator key → revert;
- **duplicate signer**: same sig twice → counts once → revert if below threshold;
- **tampered params**: change `amount` after signing → recovered id mismatch → revert.

Use `vm.sign(pk, ethSignedHash)` in Foundry to produce signatures.

**✅ Verification Checkpoint 2:**
```bash
forge test --match-contract Claim -vvv
```
All security tests (replay, threshold, bad/duplicate signer, tampering) pass.
This is your highest-risk contract code — do not advance until every case is green.

---

## Phase 3 — `submissionId` equivalence (THE critical test)

**Goal:** guarantee the contract and the off-chain validator compute the **exact
same** `submissionId` for the same inputs. If this drifts, no signature ever
verifies and the bug is maddening to find later.

**Build:**
- Finalize the hashing in `Gate.sol` (e.g. `keccak256(abi.encode(receiver,
  debridgeId, chainIdFrom, chainIdTo, amount, nonce, autoParamsHash))`).
- Port the **identical** logic to `validator/src/utils/buildSubmissionId.ts`
  (this repo already has this file — rewrite its body to match your encoding,
  including how `autoParams` is decoded/hashed).

**Test — cross-language equivalence:**
1. In a Foundry test, log `submissionId` for a fixed set of inputs
   (`emit log_bytes32(id)` or write to a file via `vm.writeFile`).
2. In a Node test (`validator`), call `buildSubmissionId` with the **same** inputs.
3. Assert the two strings are identical.

A simple harness:
```solidity
// contracts/test/SubmissionId.t.sol  — prints id for known fixtures
function test_PrintIds() public {
    bytes32 id = gate.computeSubmissionId(/* fixed fixture */);
    emit log_named_bytes32("id", id);
}
```
```ts
// validator/test/buildSubmissionId.spec.ts
expect(buildSubmissionId(fixture)).toEqual(EXPECTED_FROM_FORGE);
```

**✅ Verification Checkpoint 3:**
For at least **3 fixtures** (incl. one with `autoParams` payload, one without),
the Solidity-computed id and the TS-computed id are byte-for-byte equal. Lock
these fixtures into a permanent regression test in both projects.

---

## Phase 4 — Minimal validator (scan → recompute → sign → store)

**Goal:** a single validator that watches the source chain and produces a valid
signature for each `Sent` event. Strip the node down to the essentials first.

**Build (adapt this repo under `validator/`):**
1. **Swap the ABI/event**: replace `assets/DeBridgeGate.json` with your `Gate`
   ABI; change `getPastEvents('Sent', …)` to your event name if different
   (`AddNewEventsAction.getEvents`).
2. **Point config** at local chains in `config/chains_config.json`:
   ```json
   [{ "chainId": 1337, "name": "SRC", "debridgeAddr": "0x...gate",
      "firstStartBlock": 1, "provider": "http://127.0.0.1:8545",
      "interval": 2000, "blockConfirmation": 1, "maxBlockRange": 50 }]
   ```
   > For local `anvil` you'll want `blockConfirmation` small. Note the repo's
   > `StartScanningService` enforces `blockConfirmation > 8` and `maxBlockRange >= 50`
   > — relax those guards in dev or set values above them.
3. Use your **Phase 3** `buildSubmissionId.ts`.
4. Keep `SignAction` (signs `submissionId` with the keystore key). Generate a
   keystore via `generate-keystore/`.
5. For the first run you can **disable** Arweave/deBridge-API uploads and just
   log/persist the signature.

**Test:**
- Start one chain + the validator.
- `send()` a transfer via `cast`/a script.
- Watch logs: event scanned → `submissionId` recomputed (matches emitted) →
  `status NEW → SIGNED` → signature stored in Postgres `submissions` table.
- Negative test: feed a tampered `rawEvent` (or point at a lying RPC) and confirm
  `SubmissionIdValidationService` rejects it.

**✅ Verification Checkpoint 4:**
```sql
SELECT "submissionId", status, signature FROM submissions;
```
Row exists with `status = SIGNED` and a non-null `signature`, and the validator's
logged recomputed id equals the on-chain emitted id.

---

## Phase 5 — Keeper + FIRST END-TO-END TRANSFER 🎉

**Goal:** move a token from chain 1337 to chain 1338, verified by a real
validator signature.

**Build (`keeper/index.ts`):**
- Read the stored signature(s) for a `submissionId` (from Postgres directly, or
  from the sig-store API once it exists).
- Once ≥ threshold (=1 for now) signatures exist, build the `claim()` call with
  the submission params + signatures and submit it to the **target** chain
  (1338) with a funded key.
- Handle nonce/gas/retry minimally.

**Test (the money test):**
1. Deploy `Gate` on **both** chains; register your one validator + threshold 1 on
   the target gate; pre-fund the target gate (or use mint-on-claim).
2. `send()` 100 tokens on 1337 → validator signs → keeper claims on 1338.
3. Assert receiver balance on 1338 increased by the expected amount.
4. Assert `executed[submissionId] == true` on 1338.
5. Re-run the keeper → second claim reverts (replay guard holds end-to-end).

**✅ Verification Checkpoint 5:**
```bash
cast call <token1338> "balanceOf(address)" <receiver> --rpc-url http://127.0.0.1:8546
# balance reflects the bridged amount; a repeat claim reverts
```
**You now have a working bridge for the happy path with one validator.** Everything
after this is hardening and scaling.

---

## Phase 6 — Harden the validator (turn the prototype into the real node)

**Goal:** re-enable and verify the safety machinery this repo already implements.

**Build / re-enable (mostly already present — verify each works):**
- **Sequential nonce enforcement** (`NonceControllingService`): test
  `MISSED_NONCE` (skip a nonce) and `DUPLICATED_NONCE` (replay an event) →
  scanner pauses / RPC flagged as designed.
- **Finality** (`blockConfirmation`): confirm the node only signs events buried
  by N confirmations; simulate a reorg on `anvil` and confirm it doesn't sign a
  vanished event.
- **Multi-RPC failover** (`ChainProvider` + `Web3Service`): add a dead RPC first
  in the list; confirm rotation to the healthy one and the chainId mismatch guard.
- **Resumability**: kill and restart the node; confirm it resumes from
  `supported_chains.latestBlock` (the DB cursor) without re-signing or missing
  events.
- **Operator API** (`AppController`): test `POST /rescan`, `/chain/scanning/pause`
  and `/start`.

**Test each as an isolated scenario** (one checkpoint per mechanism).

**✅ Verification Checkpoint 6:**
A short test log/checklist where each safety mechanism is individually
demonstrated: missed-nonce pause, duplicate-nonce pause, finality delay,
RPC failover, restart-resume, rescan. All behave as specified.

---

## Phase 7 — N validators + threshold, then dockerize

**Goal:** real trust model — multiple independent validators, threshold > 1.

**Build:**
- Run **3 validator instances**, each with its **own keystore key**, each writing
  to the shared sig-store (or their own DB + a sig aggregator).
- Stand up the **sig-store API** (replace/extend `DebrdigeApiService`): accepts
  `{submissionId, signer, signature}`, dedupes by signer, serves all sigs for an
  id. Re-enable `UploadToApiAction`.
- Set target gate `threshold = 2`. Register all 3 validator addresses.
- Keeper now waits for **≥ 2 distinct** valid signatures before claiming.
- **Dockerize** using this repo's `docker-compose.yml` as the template (Postgres +
  node; add your sig-store + keeper services).

**Test:**
- Bridge a transfer; keeper succeeds only after 2 of 3 validators sign.
- Take 1 validator offline → still works (2/3). Take 2 offline → keeper waits,
  no claim (safety holds).
- Feed 1 validator a malicious/lying RPC → it refuses to sign (id mismatch);
  honest 2 still reach threshold.

**✅ Verification Checkpoint 7:**
Transfer completes with 2-of-3 signatures; degrades safely at 1-of-3
(no execution); a single Byzantine/faulty validator cannot block or forge.

---

## Phase 8 — Asset registry + wrapped tokens (bridging *new* assets)

**Goal:** support assets that don't yet exist on the target chain (mirror
`CheckAssetsEventAction` + `ConfirmNewAsset`).

**Build:**
- Contract: `assetId (debridgeId) → {nativeChainId, nativeAddress}` registry;
  deploy a wrapped ERC-20 on first claim keyed by a signed **`deployId`**
  (`hash(prefix, debridgeId, keccak(name), keccak(symbol), decimals)` — exactly
  as in `CheckAssetsEventAction`).
- Validator: re-enable `CheckAssetsEventAction` to read native token metadata
  (`getDebridge`/`getNativeInfo` on EVM) and sign the `deployId`.
- Keeper: deploy/register the wrapped token using collected `deployId` sigs, then
  proceed with the normal claim.

**Test:**
- Bridge a brand-new token → wrapped token auto-deploys on target with correct
  name/symbol/decimals → receiver gets wrapped balance.
- Bridge the **same** token again → no redeploy (`ASSETS_ALREADY_CREATED` path),
  reuses the existing wrapped token.
- Bridge the wrapped token **back** → burns on target, unlocks original on source.

**✅ Verification Checkpoint 8:**
Round-trip a never-before-seen asset (A→B mints wrapped, B→A unlocks original)
with correct metadata and no double-deploy.

---

## Phase 9 — Testnet soak, chaos, and audit prep

**Goal:** confidence before anything touches mainnet/real value.

**Do:**
- Deploy gates on **real testnets** (e.g. Sepolia + an L2 testnet); run the full
  validator set + keeper for days.
- **Chaos**: kill validators, throttle/blackhole RPCs, force reorgs, replay old
  events, submit malformed `autoParams`, spam duplicate claims.
- **Metrics/alerting**: wire `MonitoringModule` + Sentry + `notifyError`; alert on
  stuck nonces, scanner pauses, signature shortfalls.
- **Audit**: focus auditors on `claim()` signature verification, replay guard,
  threshold math, and the `submissionId`/`deployId` hashing equivalence.

**✅ Verification Checkpoint 9 (go/no-go):**
- N-day continuous testnet run with zero stuck/duplicate/forged transfers.
- All chaos scenarios fail safe (never an unauthorized release of funds).
- Audit findings resolved. Only then consider mainnet + governance-managed
  validator onboarding.

---

## Master checklist (print this)

- [ ] **P0** Two local chains reachable (`cast chain-id`).
- [ ] **P1** `send()` locks + emits `Sent`; nonce increments (`forge test`).
- [ ] **P2** `claim()` passes replay / threshold / bad-signer / tamper tests.
- [ ] **P3** Solidity `submissionId` == TS `buildSubmissionId` for 3+ fixtures.
- [ ] **P4** One validator scans → recomputes → signs; row is `SIGNED` in DB.
- [ ] **P5** **First end-to-end transfer**; repeat claim reverts.
- [ ] **P6** Nonce/finality/RPC-failover/resume/rescan each verified.
- [ ] **P7** 2-of-3 threshold works; degrades & resists Byzantine safely.
- [ ] **P8** New-asset round-trip (wrapped mint ↔ original unlock), no double-deploy.
- [ ] **P9** Testnet soak + chaos pass; audit clean; go/no-go met.

---

## Where the effort really goes (set expectations)

- **Reused from this repo (low effort):** chain scanning + block paging, the
  per-chain DB cursor/resume, sequential-nonce logic, multi-RPC failover, the
  multi-stage cron status pipeline, key handling, operator API.
- **Net-new, high-care work:** the **Gate contracts** (`send`/`claim` + threshold
  signature verification + replay guard), the **exact hashing scheme** and its
  cross-language equivalence test, the **keeper**, and the **validator-set
  governance**. Spend your review/audit budget here.

> First milestone to aim for: **Phase 5** (one validator, one end-to-end transfer
> on two local chains). Everything before it is small and fast; reaching it proves
> the whole architecture works on your machine.
```
