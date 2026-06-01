# SelendraBridge — Architecture, How It Works, and a Plan to Build Your Own Bridge

> bridge architecture it participates in, and a concrete plan for designing and
> building your own custom bridge using the same patterns.

---

## Table of Contents

1. [What this repository is](#1-what-this-repository-is)
2. [Big-picture bridge architecture](#2-big-picture-bridge-architecture)
3. [How a cross-chain transfer flows end-to-end](#3-how-a-cross-chain-transfer-flows-end-to-end)
4. [What the validator node actually does (this repo)](#4-what-the-validator-node-actually-does-this-repo)
5. [Codebase map — modules and responsibilities](#5-codebase-map--modules-and-responsibilities)
6. [The submission lifecycle (state machine)](#6-the-submission-lifecycle-state-machine)
7. [Core security mechanisms](#7-core-security-mechanisms)
8. [Deployment topology](#8-deployment-topology)
9. [Plan: building your own custom bridge](#9-plan-building-your-own-custom-bridge)

---

## 1. What this repository is

This repo is the **SelendraBridge validation node** — software run by validators who
were elected by SelendraBridge governance. It is **NOT** the bridge smart contracts
and **NOT** the relayer/executor. It is the **off-chain oracle/attestation
layer** that watches chains and signs valid cross-chain messages.

SelendraBridge as a whole is a **cross-chain interoperability and liquidity transfer
protocol** that enables:
- cross-chain composability of smart contracts (arbitrary message passing),
- cross-chain swaps,
- bridging of arbitrary assets,
- NFT interoperability.

The node's job, in one sentence (from the README):

> All active validators listen for events emitted by transactions passing
> through the SelendraBridge smart contract; once an event reaches finality, the
> validator signs the unique id of the event with its private key and stores
> the signature (to the SelendraBridge API and/or Arweave). To execute on the target
> chain, a keeper collects the minimum required validator signatures and submits
> them with the transaction parameters to the SelendraBridge gate contract on the
> target chain, which verifies the signatures and executes the message.

So this is a classic **external-validator (MPC-style / multi-sig oracle) bridge
design**, not an optimistic or light-client/ZK bridge.

---

## 2. Big-picture bridge architecture

A bridge of this style has four logical layers. This repo implements **only the
Validator layer**.

```
            SOURCE CHAIN                                        TARGET CHAIN
 ┌───────────────────────────────┐                  ┌───────────────────────────────┐
 │  SelendraBridgeGate contract         │                  │  SelendraBridgeGate contract         │
 │  • send() locks/burns asset    │                  │  • claim() verifies N sigs     │
 │  • emits Sent(submissionId,…)  │                  │    then mints/unlocks + exec   │
 └───────────────┬───────────────┘                  └───────────────▲───────────────┘
                 │ event                                             │ tx + signatures
                 ▼                                                   │
 ┌─────────────────────────────────────────────┐                    │
 │  VALIDATOR NODES  (THIS REPO)                 │                   │
 │  1. scan source chain for `Sent` events       │                  │
 │  2. recompute submissionId, validate          │                  │
 │  3. sign submissionId with validator key       │                 │
 │  4. publish signature → SelendraBridge API / Arweave │                 │
 └───────────────┬───────────────────────────────┘                 │
                 │ signatures stored                                 │
                 ▼                                                    │
 ┌─────────────────────────────────────────────┐                    │
 │  KEEPER / EXECUTOR  (separate service)        │ ───────────────────┘
 │  • collects ≥ threshold signatures            │
 │  • submits claim() to target chain            │
 └───────────────────────────────────────────────┘
```

Key design properties:

- **Trust model**: security comes from an honest majority of the elected
  validator set. The target-chain contract requires a **minimum threshold of
  signatures** before it executes anything.
- **`submissionId`** is the unique, deterministic identifier of a cross-chain
  message. It is a hash of all the transfer parameters. Every validator
  **independently recomputes it** from on-chain data and only signs if it
  matches what the contract emitted. This is the crux of the security.
- **Validators do not move funds.** They only attest. A separate keeper submits
  the execution transaction, paying gas and collecting the execution fee.

---

## 3. How a cross-chain transfer flows end-to-end

1. **User calls `send()`** on the `SelendraBridgeGate` contract on the source chain
   (locks/burns tokens, optionally attaches a message / external call).
2. The contract **emits a `Sent` event** containing `submissionId`, `SelendraBridgeId`
   (asset id), `amount`, `receiver`, `chainIdTo`, `nonce`, and `autoParams`
   (execution fee, flags, fallback address, payload data).
3. **Validator nodes scan** the source chain, see the `Sent` event after the
   configured number of block confirmations (finality).
4. Each validator **recomputes the `submissionId`** from the raw event and
   compares it to the emitted one (`buildSubmissionId`).
5. If valid (and the **nonce** is sequentially correct), the validator **signs
   the `submissionId`** with its ECDSA private key.
6. The validator **uploads the signature** to the SelendraBridge API and optionally
   **Arweave** (permanent storage) for redundancy/decentralization.
7. A **keeper** queries the API for a `submissionId`, gathers signatures from
   ≥ threshold validators, and calls **`claim()`** on the target chain with the
   parameters + signatures.
8. The **target `SelendraBridgeGate` verifies** the signatures against the known
   validator set and, if the threshold is met, **mints/unlocks** the asset to
   the receiver and **executes any attached call**.

A parallel flow exists for **new asset registration** (`ConfirmNewAsset`): the
first time an asset crosses a bridge, validators compute and sign a `deployId`
(hash of `SelendraBridgeId` + token name + symbol + decimals) so the target chain can
deploy the wrapped-token contract with correct metadata.

---

## 4. What the validator node actually does (this repo)

The node is a **NestJS (TypeScript, Fastify) application** backed by
**PostgreSQL** (via TypeORM). It runs two kinds of work continuously:

### A. Per-chain scanners (event-driven intake)
Started in `StartScanningService.onModuleInit()`:
- For each configured chain it ensures a `supported_chains` DB row exists and
  validates RPC connectivity / chainId.
- Then `ChainScanningService.start(chainId)` registers a `setInterval` (period =
  `chain.interval`) that repeatedly calls `AddNewEventsAction.action(chainId)`.

`AddNewEventsAction`:
- For **EVM chains**: loads the `SelendraBridgeGate` ABI, computes the confirmed block
  (`latestRpcBlock − blockConfirmation`), and pages through blocks in
  `maxBlockRange` chunks calling `getPastEvents('Sent', …)`.
- Each raw event → `TransformService.generateSubmissionFromSentEvent()` →
  a `SubmissionEntity`.
- Submissions are sorted by `nonce` and handed to
  `SubmissionProcessingService.process()`.
- A per-chain in-memory **lock** prevents overlapping interval runs.
- For **Solana**: delegates to `SolanaReaderService.syncTransactions()` which
  consumes events from a gRPC stream (separate Rust services, see below).

`SubmissionProcessingService.processNewTransfers()` runs the validation gauntlet
for each submission:
- skip if already in DB (idempotency),
- **nonce validation** (`NonceControllingService.validateNonce`) — must be
  exactly `maxNonce + 1`; detects `MISSED_NONCE` and `DUPLICATED_NONCE`,
- **submissionId validation** (`SubmissionIdValidationService.validate`) —
  recomputes the id and compares,
- on success: **save** the submission with `status = NEW` and advance the
  chain's `latestBlock` / `latestNonce`,
- on failure: mark the current RPC provider bad, notify the API, and after
  `maxAttemptsSubmissionIdCalculation` failures **pause** scanning that chain.

### B. Cron jobs (the signing & publishing pipeline)
Defined in `JobService` (`@nestjs/schedule` cron):

| Job | Schedule | What it does |
|-----|----------|--------------|
| `SignAction` | every 3s | finds `status = NEW` submissions, signs `submissionId` with the keystore account, sets `status = SIGNED` + stores `signature`. |
| `UploadToApiAction` | every 3s | pushes `SIGNED` + `apiStatus = NEW` submissions (and confirmed assets) to the SelendraBridge API in pages of 100, records `externalId`. |
| `CheckAssetsEventAction` | every 3s | for each submission's `SelendraBridgeId`, looks up native token info on-chain (EVM `getSelendraBridge`/`getNativeInfo` or Solana gRPC), computes & signs a `deployId`, saves a `ConfirmNewAssetEntity`. |
| `UploadToArweaveAction` | every 3s | uploads signed submissions/assets to Arweave via Turbo/Bundlr (permanent, censorship-resistant signature store). |
| `StatisticToApiAction` | every 1m | reports validation progress/health to the API. |

### C. Operator HTTP API (`AppController`, Fastify + Swagger at `/api/docs`)
- `POST /login` (JWT) — operator auth.
- `POST /rescan` — re-scan a chain from/to a block (recovery).
- `GET /chains`, `GET /chains/config` — inspect supported chains.
- `GET /chain/scanning/start|pause|status` — control scanners at runtime.

---

## 5. Codebase map — modules and responsibilities

```
SelendraBridge_node/src/
├── main.ts                         # bootstrap: Fastify + Swagger, Sentry, BigInt JSON patch
├── AppModule.ts                    # wires all modules; TypeORM(Postgres); entities
│
├── entities/
│   ├── SubmissionEntity.ts         # the core record: submissionId, chains, amount, statuses…
│   ├── ConfirmNewAssetEntity.ts    # new-asset registration (deployId, token metadata)
│   └── SupportedChainEntity.ts     # per-chain cursor: latestBlock / latestNonce / …
│
├── enums/                          # status enums: Submision/Upload/Bundlr/AssetsStatus…
│
├── modules/
│   ├── chain/
│   │   ├── config/                 # ChainConfigService loads config/chains_config.json
│   │   │   └── models/             # EvmChainConfig, SolanaChainConfig, ChainProvider (multi-RPC failover)
│   │   └── scanning/services/
│   │       ├── ChainScanningService.ts      # start/pause/status intervals per chain
│   │       ├── AddNewEventsAction.ts         # EVM event paging + dispatch
│   │       ├── SolanaReaderService.ts        # Solana gRPC stream consumer
│   │       ├── SubmissionProcessingService.ts# nonce + submissionId gauntlet, persistence
│   │       ├── NonceControllingService.ts    # sequential-nonce enforcement
│   │       ├── SubmissionIdValidationService.ts # recompute & compare submissionId
│   │       └── TransformService.ts           # raw event → SubmissionEntity (EVM & Solana)
│   │
│   ├── jobs/
│   │   ├── JobService.ts            # cron registrations
│   │   └── services/
│   │       ├── StartScanningService.ts        # onModuleInit bootstrap of scanners
│   │       └── actions/{SignAction, UploadToApiAction, CheckAssetsEventAction,
│   │                     UploadToArweaveAction, StatisticToApiAction}.ts
│   │
│   ├── web3/Web3Service.ts          # Web3 provider pool w/ health-check + chainId validation
│   ├── external/
│   │   ├── SelendraBridge_api/            # DebrdigeApiService: auth + upload signatures/assets/progress
│   │   └── arweave/TurboService.ts  # permanent signature storage
│   ├── solana-events-reader/        # SolanaEventsReaderService: gRPC client wrapper
│   ├── api/                         # AppController + auth (JWT) + RescanService
│   └── monitoring/                  # health/metrics
│
├── utils/
│   ├── buildSubmissionId.ts         # deterministic submissionId hashing (EVM + Solana autoParams)
│   ├── createU256.ts / createSolanaPublicKey.ts / getEvmTokenName|Symbol.ts
│   └── readConfiguration.ts
└── datafixes/                       # one-off DB migrations/fixes
```

Supporting infrastructure:
- `config/chains_config.json` — the per-chain RPC + gate address + scanning params.
- `generate-keystore/` — produces the validator's ETH keystore + password.
- `generate-arweave-wallet/` — produces the Arweave wallet for permanent storage.
- `docker-compose.yml` — Postgres + Solana gRPC services + the node.

---

## 6. The submission lifecycle (state machine)

A `SubmissionEntity` carries **four independent status fields** so each pipeline
stage advances on its own cron cadence:

```
                 scan + validate
  Sent event ───────────────────────►  saved row
                                         status      = NEW
                                         apiStatus   = NEW
                                         bundlrStatus= NEW
                                         assetsStatus= NEW
                                            │
            SignAction (every 3s)           │ sign(submissionId)
                                            ▼
                                         status      = SIGNED   (+ signature)
                            ┌───────────────┼───────────────────────────┐
   UploadToApiAction (3s)   │   UploadToArweaveAction (3s)   CheckAssetsEventAction (3s)
            │               │               │                            │
            ▼               │               ▼                            ▼
   apiStatus = UPLOADED     │     bundlrStatus = UPLOADED      assetsStatus = ASSETS_CREATED
   (+ externalId)           │     (+ bundlrTx)                 (creates ConfirmNewAssetEntity)
```

Key fields on `SubmissionEntity`:
`submissionId` (PK), `txHash`, `chainFrom`, `chainTo`, `SelendraBridgeId`,
`receiverAddr`, `amount`, `nonce`, `blockNumber`, `rawEvent` (the full event JSON
needed to recompute the id), `signature`, `externalId`, and the four status enums.

The **`supported_chains`** table is the durable cursor — `latestBlock` (EVM) /
`latestNonce` + Solana slot fields — so the scanner resumes exactly where it left
off after a restart.

---

## 7. Core security mechanisms

These are the parts you must get right in *any* bridge of this design:

1. **Deterministic `submissionId` recomputation** (`buildSubmissionId.ts`).
   The validator never trusts the emitted id blindly — it rebuilds the
   `keccak`/hash from receiver, SelendraBridgeId, source/target chain, amount, nonce,
   and decoded `autoParams`, and signs **only on an exact match**. A mismatch
   means a buggy/malicious RPC; the node marks the provider bad and, after
   `maxAttemptsSubmissionIdCalculation` strikes, pauses the chain and alerts.

2. **Sequential nonce enforcement** (`NonceControllingService`).
   Each source chain has a monotonic per-chain nonce. The node requires every
   new submission to be `maxNonce + 1`:
   - `DUPLICATED_NONCE` → a nonce already seen → **pause scanning** (possible
     reorg/fork or fraud) and notify.
   - `MISSED_NONCE` → a gap → mark RPC bad and notify (likely an RPC that
     skipped a block range).
   This prevents replay and out-of-order/missing-event attacks.

3. **Block confirmations / finality** (`blockConfirmation`).
   The node only processes up to `latestBlock − blockConfirmation` so reorgs
   don't cause it to sign transfers that later disappear.

4. **Multi-RPC failover** (`ChainProvider` + `Web3Service`).
   Each chain has a list of RPCs; failing ones are marked and rotated to the
   back of the list. On connect, the node verifies the RPC's reported chainId
   matches the gate contract's `getChainId()` (`exit(1)` on mismatch) — prevents
   accidentally validating the wrong chain.

5. **Key isolation**. The signing key lives in an encrypted keystore
   (`secrets/keystore.json`, password in `KEYSTORE_PASSWORD`) mounted as a Docker
   secret; the Arweave key is separate (`bundlr_wallet.json`).

6. **Redundant signature publication** (API **and** Arweave) so attestations
   remain available even if the centralized API is down.

---

## 8. Deployment topology

From `docker-compose.yml`:

- **`postgres`** — state store (two DBs: node DB + Solana-reader DB).
- **`solana-events-reader`** (Rust image) — subscribes to Solana, decodes
  SelendraBridge program events at `finalized` commitment, writes to Postgres.
- **`solana-grpc-service`** (Rust image) — serves those events to the node over
  gRPC (`SOLANA_GRPC_SERVICE_URL`).
- **`SelendraBridge-node`** (this app) — scanners + cron pipeline + operator API.
  Mounts `keystore.json` and `bundlr_wallet.json` as secrets and the `config/`
  dir for `chains_config.json`.

Operator setup (README): run full nodes / RPCs for each chain, fill
`chains_config.json`, set `.env` (DB creds, JWT, API creds, keystore password,
SelendraBridge program pubkeys, Solana RPC), generate keystore + Arweave wallet, get
whitelisted by governance, then `docker-compose up --build -d`.

---

## 9. Plan: building your own custom bridge

Below is a pragmatic, phased plan to build a bridge using the same
**external-validator attestation** architecture this repo demonstrates. Adapt
the threat model to your needs.

### Phase 0 — Decide the trust/verification model
Pick one; it dictates everything else:
- **External validators / multisig oracle** *(what SelendraBridge uses)* — simplest to
  ship, security = honest validator majority + threshold signatures. **Recommended
  starting point.**
- **Light client / ZK** — trustless but expensive to build (on-chain header
  verification or validity proofs).
- **Optimistic** — cheaper, adds a fraud-proof challenge window (latency).

The rest of this plan assumes the external-validator model.

### Phase 1 — On-chain contracts (the part NOT in this repo)
Build a `Gate`-style contract deployed on **every** supported chain:
- **`send(token, amount, chainIdTo, receiver, autoParams)`**
  - lock (native/canonical asset) or burn (wrapped asset);
  - compute a deterministic **`submissionId = hash(receiver, assetId, chainFrom,
    chainTo, amount, nonce, autoParams)`**;
  - increment a per-(chainTo) **nonce**;
  - `emit Sent(submissionId, …all params…)`.
- **`claim(params…, signatures[])`**
  - recompute `submissionId` from params;
  - recover signers from `signatures`, check each is in the validator set, and
    require **`≥ threshold`** distinct valid signers;
  - guard against replay (`mapping(submissionId => bool) executed`);
  - mint/unlock to `receiver`, then execute any attached call (`autoParams.data`).
- **Validator set management** — governance-controlled add/remove validators and
  threshold; consider a timelock.
- **Asset registry** — `assetId → {nativeChainId, nativeAddress}` and wrapped
  token deployment keyed by a signed `deployId` (mirror `CheckAssetsEventAction`).

Write extensive tests for: signature verification, threshold edge cases, replay,
reorg-safety assumptions, and malformed `autoParams`.

### Phase 2 — Fork/adapt this validator node
This repo is a strong template. Concretely:
1. **Swap the ABI & event**: replace `assets/SelendraBridgeGate.json` and the
   `getPastEvents('Sent', …)` name with your gate/event.
2. **Rewrite `buildSubmissionId.ts`** to match your contract's hashing **byte-for-
   byte** — this MUST be identical on-chain and off-chain or nothing verifies.
3. **Reuse as-is** (they're bridge-agnostic): `ChainScanningService`,
   `AddNewEventsAction` paging loop, `SubmissionProcessingService`,
   `NonceControllingService`, `Web3Service` multi-RPC pool, the
   `SubmissionEntity` + status state machine, and the cron `SignAction`.
4. **Configure chains** via `chains_config.json` (gate address, RPCs,
   `blockConfirmation`, `maxBlockRange`, `interval`, `firstStartBlock`).
5. **Decide non-EVM support**: drop the Solana modules entirely if EVM-only, or
   build an analogous reader per ecosystem (the Solana path here is a good
   reference for "stream events over gRPC from a chain-specific decoder").

### Phase 3 — Signature storage / aggregation
Replace `DebrdigeApiService` with your own:
- **Option A (centralized API)**: a service that collects `{submissionId,
  signature, signer}` from validators and serves them to keepers. Fast, simple,
  but a liveness dependency.
- **Option B (decentralized)**: keep the **Arweave/IPFS** publication
  (`UploadToArweaveAction` is a ready pattern) and have keepers read directly.
- **Option C (gossip)**: validators form a p2p network and aggregate signatures
  among themselves (most decentralized, most work).

### Phase 4 — Keeper / executor service (also NOT in this repo)
A separate service that:
- watches for submissions with enough signatures,
- builds and submits the `claim()` tx on the target chain,
- manages gas, nonce, retries, and collects the execution fee.
Keepers are permissionless in a good design — anyone can execute.

### Phase 5 — Operations & safety
- **Monitoring/alerting** (reuse `MonitoringModule`, Sentry, `notifyError`).
- **Operator API** for `rescan`, pause/resume per chain (reuse `AppController`).
- **Finality tuning** per chain (`blockConfirmation`) — get this wrong and you
  sign transfers that reorg away.
- **Key management** — HSM/KMS for production validator keys, not a flat
  keystore.
- **Validator onboarding/governance** — how the set is elected and rotated.

### Phase 6 — Audits & testnet
- Audit the contracts (signature verification + replay are the highest-risk
  code).
- Run the full validator set + keeper on testnets across all target chains,
  including reorg and RPC-failure chaos testing, before mainnet.

### Minimum viable custom bridge — checklist
- [ ] `Gate` contract (`send`/`claim`) deployed on each chain, with matching
      `submissionId` hashing.
- [ ] Governance-managed validator set + threshold.
- [ ] N validator nodes (this repo, adapted) each with an isolated signing key.
- [ ] Signature store (API or Arweave/IPFS).
- [ ] Keeper service submitting `claim()`.
- [ ] Per-chain finality config + nonce sequencing + multi-RPC failover.
- [ ] Replay protection on-chain (`executed[submissionId]`).
- [ ] Monitoring, rescan tooling, audits, testnet soak.

### What you get "for free" by reusing this repo
The non-trivial, already-solved engineering here: resumable per-chain scanning
with a DB cursor, block-range paging, sequential-nonce enforcement, deterministic
id recomputation, multi-RPC health/failover, an idempotent multi-stage status
pipeline on independent crons, encrypted key handling, and an operator control
API. Reuse these; focus your novel effort on the **contracts**, the **exact
hashing scheme**, and the **keeper**.

---

### TL;DR
- This repo = the **off-chain validator** of an external-validator bridge. It
  **scans → validates (recompute id + nonce) → signs → publishes** signatures.
- Security rests on **threshold signatures from an honest validator majority**
  plus **independent on-chain-data recomputation of `submissionId`**.
- To build your own: write the **Gate contracts** (`send`/`claim` + signature
  threshold + replay guard) and a **keeper**, then **adapt this node** (swap the
  ABI/event and the `buildSubmissionId` hashing) for the validator layer.
```
