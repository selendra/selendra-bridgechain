# On-Chain Swap for Bridge Tokens — Step-by-Step Plan

> A **same-chain** swap layer that lets a user exchange any bridge-registered
> ERC-20 for any other on the same chain, priced against **one stablecoin as the
> core unit of account**, with each token's throughput hard-capped by its locked
> reserve. Built as a standalone `SwapPool` contract per chain, deliberately
> structured so a **cross-chain router** can be layered on later without rework.
>
> Companion to [`BRIDGE_BUILD_PLAN.md`](./BRIDGE_BUILD_PLAN.md). Same rules:
> every phase ends with a **✅ Verification Checkpoint** — a concrete test that
> proves the step before you move on. Do not skip checkpoints. **This is a plan;
> no contract code is written yet.**

---

## Decisions locked (from the design questions)

| Question | Choice | Consequence |
|----------|--------|-------------|
| **Swap scope** | **Same-chain AMM now, cross-chain later** | Build a local `SwapPool` on each chain first; keep swap logic isolated behind a clean interface so a future `SwapRouter` can combine it with `Gate.send()`/`claim()`. |
| **Pricing model** | **Pegged / fixed price** | An admin/oracle sets each token's USD price. Swaps execute at that rate. Prices do **not** self-correct, so the trust + freshness of the price source is the #1 risk (see Security). |
| **Core price unit** | **One stablecoin** | The stablecoin *is* the unit of account: its price is fixed at `1.0` (and immutable). Every other token is priced in stablecoin/USD. |
| **Max swap** | **Locked reserve of the output token** | A swap that would pay out more than `reserve[tokenOut]` reverts. This is the literal "maximum swap up to token lock." |
| **Liquidity model** | **Protocol-owned** | The owner seeds each pool. No LP tokens / no public `addLiquidity` in v1 — removes a whole class of share-accounting attacks. Can be added later. |

---

## Guiding principles

1. **Same-chain and atomic first.** A swap is one transaction on one chain — no
   validators, no signatures, no keeper. That machinery only re-enters the
   picture in the (later) cross-chain phase.
2. **Reuse the Gate's security idioms.** The `Gate` already establishes the
   patterns this repo trusts: `SafeERC20`, checks-effects-interactions, two-step
   ownership, a guardian + `paused` circuit breaker, custom errors, rich events.
   `SwapPool` copies them so the audit surface is familiar.
3. **The stablecoin is the anchor, not necessarily the router.** With pegged
   pricing, `TokenX → TokenY` is priced directly through USD in one hop; the
   stablecoin does not have to be physically traversed. It is the *pricing hub*
   (unit of account) and also just another swappable token.
4. **Internal reserve accounting.** Track `reserve[token]` in storage rather than
   trusting `balanceOf`. This defends against donation/inflation griefing and
   lets us detect fee-on-transfer tokens by balance delta.
5. **Rounding always favors the pool.** Integer division rounds the *output*
   down. No sequence of swaps can extract value through dust.
6. **Design for the router you haven't built.** Every v1 choice (explicit `to`
   recipient, isolated `swap`, quote view) is made so a cross-chain executor can
   call it. Nothing in v1 blocks Phase E.

---

## The pegged-price model (the math)

Let the stablecoin define the unit. Prices are fixed-point, scaled by
`PRICE_ONE = 1e18`:

- `price[stable] = PRICE_ONE` (== 1.0, **immutable**)
- `price[WETH]   = 3180e18`, `price[USDC] = 1e18`, etc. (admin/oracle-set)

Tokens can have different decimals (USDC 6, WETH 18), so convert **through USD**:

```
usdValue(token, amt) = amt * price[token] / 10^decimals[token]      // 1e18-scaled USD

amountOut = usdValue(tokenIn, amountIn) * 10^decimals[tokenOut] / price[tokenOut]

# fully expanded:
amountOut = amountIn * price[tokenIn]  * 10^dec[tokenOut]
          ------------------------------------------------   (integer div, rounds DOWN)
                   price[tokenOut] * 10^dec[tokenIn]
```

Optional swap fee (default **0 bps**, designed-in): take the fee on the USD value
before converting to `tokenOut`. **As built:** the withheld value simply stays in
the pool as retained reserve (reserveIn grows by the full input, reserveOut shrinks
by only the net output); the owner captures it via `withdrawLiquidity`, so there is
no separate fee balance to sweep. The fee rounds **up** against the user.

**The lock / max-swap rule:**

```
require(amountOut <= reserve[tokenOut], ExceedsLock);   // can never drain a pool
reserve[tokenIn]  += amountInReceived;                  // effects...
reserve[tokenOut] -= amountOut;
// ...then interactions: pull tokenIn, push tokenOut
```

Because the cap is on the *output* reserve, "max swap for token T" ==
`reserve[T]` (in T units) == `reserve[T] * price[T] / 10^dec[T]` in USD.

**Worked example** (USDC 6-dec, WETH 18-dec, price WETH = 3180 USDC):
swap `1 WETH` (`1e18`) → USDC:
`out = 1e18 * 3180e18 * 10^6 / (1e18 * 10^18) = 3180 * 10^6 = 3180.000000 USDC`. ✅

---

## Contract spec — `contracts/src/SwapPool.sol` (one per chain)

> Sketch only — signatures + behavior, no bodies. Written in Phase A.

**Roles / governance** (mirrors `Gate.sol`)
- `owner` + `pendingOwner` — two-step `transferOwnership` / `acceptOwnership`.
- `oracle` — the only role allowed to call `setPrice` (may equal `owner`, but
  separable so a low-trust price feeder can't also move liquidity).
- `guardian` + `bool paused` — `pause()` (owner or guardian), `unpause()` (owner
  only); `whenNotPaused` on `swap`.

**Token registry / state**
```solidity
struct TokenInfo {
    bool    listed;
    uint8   decimals;    // cached at listing
    uint256 price;       // USD price, 1e18-scaled; stable == PRICE_ONE, immutable
    uint256 reserve;     // internal accounting (the "lock")
}
mapping(address token => TokenInfo) public tokens;
address public stable;                 // the core-price token
uint256 public constant PRICE_ONE = 1e18;
uint16  public feeBps;                 // default 0 (fees accrue into reserves)
uint16  public maxPriceDeviationBps;   // per-update price cap (anti-fat-finger)
```

**Admin / liquidity**
- `listToken(address token, uint256 price)` — cache `decimals` via
  `IERC20Metadata`, set `listed`, seed `price` (the stable is listed once at
  construction with `PRICE_ONE`).
- `setPrice(address token, uint256 newPrice)` — **oracle-only**; rejects the
  stable; enforces a **max deviation** guard (e.g. ≤ X% per update) + emits
  `PriceSet`. (See Security for why this guard matters.)
- `delistToken(address token)` — owner; only when `reserve == 0`.
- `seedLiquidity(address token, uint256 amount)` — owner pulls tokens in,
  `reserve += received` (balance-delta measured).
- `withdrawLiquidity(address token, uint256 amount, address to)` — owner;
  `reserve -= amount` then transfer. For rebalancing / decommission / fee capture.
- `maxSwapOut(address token)` — view; returns the current lock (reserve) and its
  USD value (PRICE_ONE-scaled).

**Core**
```solidity
function quote(address tokenIn, address tokenOut, uint256 amountIn)
    external view returns (uint256 amountOut);     // pure pricing, no cap check

function swap(
    address tokenIn,
    address tokenOut,
    uint256 amountIn,
    uint256 minAmountOut,     // slippage / stale-price guard for the caller
    address to
) external whenNotPaused nonReentrant returns (uint256 amountOut);
```
`swap` invariants: both listed, `tokenIn != tokenOut`, `amountIn > 0`,
`amountOut > 0`, `amountOut >= minAmountOut`, `amountOut <= reserve[tokenOut]`.
CEI order: compute → update reserves + emit → `safeTransferFrom(in)` (measure
delta) → `safeTransfer(out)`. `nonReentrant` as belt-and-suspenders.

**Events**: `TokenListed`, `PriceSet`, `LiquiditySeeded`, `LiquidityWithdrawn`,
`Swapped(user, tokenIn, tokenOut, amountIn, amountOut, to)`, `FeesSwept`, plus the
governance/pause events copied from `Gate`.

**Errors**: `NotOwner`, `NotOracle`, `TokenNotListed`, `SameToken`, `ZeroAmount`,
`ExceedsLock(want, reserve)`, `Slippage(got, min)`, `PriceDeviationTooHigh`,
`EnforcedPause`, `StableRepriceForbidden`, `ReserveNonZero`.

---

## Security analysis (the part that matters for pegged pricing)

The audit trail on the bridge (see the `bridge-build-decisions` memory) sets the
bar. Pegged pricing moves the main risk from math to **the price source**:

1. **Stale / manipulated price (the #1 risk).** Fixed prices don't self-correct,
   so a wrong `price[token]` lets an arbitrageur drain the mispriced side up to
   its lock. Mitigations, in order of strength:
   - `minAmountOut` on every swap (caller-side slippage guard).
   - **Reserve cap** bounds the *maximum* loss per token to one pool's worth —
     never the whole contract.
   - Separate **oracle** role + **max-deviation-per-update** guard so a single
     fat-fingered or compromised price push can't 100×/÷100 a token in one tx.
   - Optional **price staleness / heartbeat** (record `lastPriceUpdate`, let
     `swap` reject if older than N seconds) — note as a v1.1 option.
2. **Update front-running.** A pending `setPrice` is visible in the mempool; a
   searcher can trade at the stale favorable price just before it lands.
   Mitigations: apply price changes behind a short **timelock/commit-reveal**, or
   `pause()` the specific market during an update. Documented; default v1 relies
   on deviation-cap + reserve-cap to bound the damage.
3. **Reentrancy.** `nonReentrant` + CEI (reserves updated & event emitted before
   any external transfer), exactly like `Gate.claim`.
4. **Fee-on-transfer / rebasing tokens.** Never trust `amountIn`; credit
   `reserve[tokenIn]` by the measured `balanceOf` delta. Bridge `TestToken` is
   standard, but be defensive; optionally reject non-standard tokens at listing.
5. **Rounding extraction.** Output rounds **down**; prove with a round-trip test
   that `swap` then reverse-`swap` never returns more than you put in.
6. **Overflow.** `amountIn * price * 10^dec` can be large; 0.8 checked math
   reverts on overflow. Document the safe input ceiling; consider `mulDiv`
   (OZ `Math.mulDiv`) to avoid intermediate overflow on 18-dec × high-price.
7. **Liquidity drain by owner.** `withdrawLiquidity` is a trusted admin power —
   same trust model as the Gate owner. Note it; a timelock/multisig is the
   protocol-wide follow-up (already a deferred item for the bridge).
8. **Circuit breaker.** Guardian can `pause` swaps instantly during an incident,
   owner resumes — identical to `Gate` so ops muscle-memory transfers.

---

## Cross-chain (Phase F — BUILT)

The same-chain `SwapPool` is the primitive. Cross-chain "swap TokenX@A → TokenY@B"
is a **composition** in `contracts/src/SwapRouter.sol`, with **zero changes to
`SwapPool` OR `Gate`**:

```
@ ChainA:  SwapRouter.swapAndBridge()
             → SwapPool.swap(TokenX → stable, to = router)
             → Gate.send(stable, amount, chainIdTo=B, receiver, autoParams=encode(wantToken=TokenY, minOut))
@ off-chain: validators sign the Sent (unchanged); keeper/executor claims
@ ChainB:  Gate.claim(...) releases stable to an executor
             → SwapPool.swap(stable → TokenY, to = finalReceiver)
```

Enablers already present or cheap to add:
- `Gate` already carries `autoParams.data` **bound into the submissionId** but
  currently unexecuted (noted in the bridge memory). That field is the natural
  place to encode the destination swap intent (`wantToken`, `minOut`).
- v1 `swap(... , address to)` takes an explicit recipient, so a router/executor
  can direct output straight to the end user.
- Routing through the **stablecoin** cross-chain means only *one* asset
  (the stable) needs bridge liquidity on every chain — the hub pays off here.

**Phase F as built.** The destination leg is **trustless with no Gate callback**:
`SwapRouter.finalize` proves delivery by checking `Gate.executed[submissionId]`.
Because `amount` and the swap intent (`finalToken, finalReceiver, finalMinOut`,
carried in `autoParams.data`) are both committed inside that id — signed by the
validators — the router can trust exactly `amount` of the stable was delivered to
it for that intent. A per-id `finalized` guard makes it idempotent; if the
destination swap can't complete (output over the pool lock, token unlisted,
slippage) it **falls back to delivering the stable**, so funds never strand. The
bridge `receiver` is the peer router (owner-registered per corridor via
`setRemoteRouter`); the end user rides in the intent. `claimAndFinalize` bundles
`Gate.claim` + the destination swap into one destination tx. Nothing in `Gate` or
`SwapPool` changed — the `computeSubmissionId`/`executed`/`tokenOf` surface was
already sufficient.

Proven by `contracts/test/SwapRouter.t.sol` (8 tests: cross-chain round trip,
two-step claim→finalize, idempotency, not-delivered revert, fallback, stable
intent, access — all 75 forge tests green) and `bridge/scripts/xswap.sh` (two live
anvils: `WETH@1337 → TT@1338`, validator-signed, `swapAndBridge → claimAndFinalize`).

**Remaining follow-ups (not blocking):** keeper auto-`finalize` after claim (today
the destination swap is a separate call / `claimAndFinalize`); a graphql-api
`remoteRouter`/corridor read view; a frontend cross-chain swap mode.

---

## Phase map

```
Phase A  SwapPool.sol: registry + pegged quote + seed + swap + reserve cap   ← core   ✅ DONE
Phase B  Governance & safety: oracle role, deviation guard, pause, fees, access tests  ✅ DONE
Phase C  Deploy script + shell e2e (anvil): seed pools, swap across decimals, hit lock ✅ DONE
Phase D  Read view: expose pools/quote in graphql-api (RPC read, like Gate.executed)   ✅ DONE
Phase E  Frontend: real Swap mode wired to wallet (quote → approve → swap)             ✅ DONE
Phase F  Cross-chain SwapRouter over Gate.send/claim + autoParams intent                ✅ DONE
```

**Build status (as of this commit):** Phases A–D shipped. `contracts/src/SwapPool.sol`
+ `contracts/test/Swap.t.sol` (27 tests) + `contracts/script/DeploySwap.s.sol` +
`bridge/scripts/swap.sh`. All **67 forge tests pass** (40 existing bridge + 27 swap);
`swap.sh` proves pegged pricing and the reserve lock on a live anvil.

**Phase D as built:** `graphql-api` gained a read-only `pools(chainId)` +
`swapQuote(chainId, tokenIn, tokenOut, amountIn)` view, driven by a repeatable
`--swap CHAINID=RPC,POOL` flag (mirrors `--gate`). Listed tokens are discovered by
replaying `TokenListed`/`TokenDelisted` logs; each token's price/reserve/decimals
come from the `tokens()` getter and `maxSwapUsd` is derived (`reserve*price/10^dec`).
`swapQuote` calls the on-chain `SwapPool.quote`. Both degrade to `null` on an
unconfigured chain or RPC/revert (never fail the query). Bindings added to
`bridge-core/src/abi.rs` (`SwapPool` + `symbol()`/`decimals()` on the ERC-20).
Proven by `bridge/scripts/swap-gql.sh` (pools + quote asserted against on-chain).

---

## Phase A — `SwapPool` core

**Goal:** list tokens, seed reserves, quote, and swap with the lock enforced.

**Build:** `contracts/src/SwapPool.sol` — registry, `PRICE_ONE`, `quote`, `swap`
(CEI + `nonReentrant`), `listToken`, `seedLiquidity`, internal reserve accounting.
Reuse `SafeERC20`; pull `IERC20Metadata` for decimals.

**✅ Checkpoint** — `contracts/test/Swap.t.sol`:
- `quote` is correct across mixed decimals (the WETH↔USDC worked example).
- `stable → token`, `token → stable`, and `token → token` all pay the pegged rate.
- swap **reverts `ExceedsLock`** when output would exceed `reserve[tokenOut]`
  (the max-swap rule) — and succeeds at exactly the reserve boundary.
- `minAmountOut` slippage revert fires.
- round-trip (swap then reverse) never returns more than input (rounding favors pool).
- `forge test` green alongside the existing 34 bridge tests.

## Phase B — Governance & safety

**Goal:** the pegged model is only as safe as its controls.

**Build:** oracle role + `setPrice` with max-deviation guard; two-step ownership;
guardian + `pause`/`unpause` (`whenNotPaused` on `swap`); `withdrawLiquidity`,
`delistToken`, optional `feeBps` + `sweepFees`; full event set.

**✅ Checkpoint** — extend `Swap.t.sol` (mirror `Security.t.sol`):
- non-oracle `setPrice` reverts; stable can't be repriced.
- a price jump beyond the deviation cap reverts.
- guardian can pause, only owner unpauses; `swap` reverts while paused.
- a `ReentrantToken` (copy the one in `Security.t.sol`) cannot reenter `swap`.
- fee math: with `feeBps` set, output drops by the fee (retained in reserves).

## Phase C — Deploy + shell e2e

**Goal:** prove it on a real anvil, the way every other subsystem is proven.

**Build:** `contracts/script/DeploySwap.s.sol` (script dir is currently empty) —
deploy stablecoin + 2 `TestToken`s, deploy `SwapPool`, list + seed. New
`bridge/scripts/swap.sh` in the style of `e2e.sh`: boot one anvil, run the deploy
script via `forge script`, execute swaps with `cast`, assert balances and that a
swap past the lock reverts.

**✅ Checkpoint:** `scripts/swap.sh` exits 0 — a decimals-crossing swap pays the
expected amount and an over-the-lock swap is rejected on-chain.

## Phase D — GraphQL read view (optional but on-brand)

**Goal:** surface pool state to the UI the same way `graphql-api` surfaces
on-chain `executed` (lazy RPC reads via a `--gate`-style `--swap CHAINID=RPC,POOL`).

**Build:** a `pools { token symbol price reserve maxSwapUsd }` query and a
`swapQuote(chainId, tokenIn, tokenOut, amountIn)` resolver that calls
`SwapPool.quote` over RPC. Read-only; no new mutations; respects the existing
depth/complexity limits.

**✅ Checkpoint:** extend `scripts/graphql.sh` (or a new `swap-gql.sh`) — `pools`
returns the seeded reserves and `swapQuote` matches the on-chain `quote`.

## Phase E — Frontend Swap mode

**Goal:** turn the existing `SwapCard` into a working **same-chain swap** using
the connected wallet (built in the recent wallet work), distinct from the bridge
flow.

**Build:** token selectors from `pools`; live `quote` on input; show
`maxSwap = reserve[tokenOut]`; slippage → `minAmountOut`; execute
`approve` → `swap` via the EIP-1193 provider (`bridge/frontend/src/wallet`).
Reuse `formatUnits` / `chainViz` in `src/data/assets.ts`.

**✅ Checkpoint:** hard-refresh the dev app, connect MetaMask to the anvil chain,
swap TokenX→USDC, watch the balance change and the reserve/max update — verified
with the headless-screenshot harness ([[headless-browser-in-wsl]]).

---

## Open defaults (change any before Phase A)

- **Stablecoin token:** reuse `TestToken` as `mUSD` (`price = PRICE_ONE`). If you
  want realistic 6-decimals, deploy it with 6 decimals — the math already
  normalizes decimals, so either works.
- **Which tokens are listed:** the same ERC-20s the `Gate` registers per chain
  (keeps "bridge tokens" and "swap tokens" the same set). v1 does **not** hard-bind
  `SwapPool` to `Gate.tokenOf`; note as an optional integrity check.
- **Fee:** `feeBps = 0` by default (pure pegged price).
- **Deviation cap / staleness:** pick concrete numbers in Phase B (suggest
  ≤ 10% per update; staleness deferred to v1.1).

---

## What this plan deliberately does **not** do (yet)

- No public LP / share tokens (liquidity is protocol-owned).
- No automatic price discovery (pegged, by decision) — a Chainlink-style oracle
  adapter is a clean v1.1 swap-in for the `oracle` role.
- No cross-chain execution (Phase F composition only).
- No changes to `Gate.sol`, the validator, or the keeper for same-chain swaps —
  they are untouched until Phase F.
