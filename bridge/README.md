# EVM ↔ EVM Bridge (Phases 0–7)

An external-validator bridge built per [`../docs/BRIDGE_BUILD_PLAN.md`](../docs/BRIDGE_BUILD_PLAN.md),
modeled on [deBridge's `DeBridgeGate`](https://github.com/debridge-finance/debridge-contracts-v1/tree/main/contracts/transfers).
On-chain gate in **Solidity**; off-chain validator + keeper + sig-store in **Rust**.

```
bridge/
├── contracts/                 # Foundry: Gate.sol, BridgeHash.sol, TestToken.sol + tests
│   └── fixtures/              # submissionId fixtures shared with Rust (Phase 3)
├── crates/
│   ├── bridge-core/          # the SACRED hashing (submissionId) + store + Gate ABI bindings
│   ├── validator/            # scan → recompute → sign → store
│   ├── keeper/               # collect ≥ threshold sigs → submit claim()
│   └── sig-store/            # Phase 7: HTTP signature store (axum)
├── docker/                   # Phase 7: compose configs + host deploy helper
├── Dockerfile                # builds validator/keeper/sig-store
├── docker-compose.yml        # sig-store + 3 validators + keeper + 2 anvils
└── scripts/
    ├── e2e.sh                # Phase 5: 2 anvils, deploy, run validator+keeper, assert a transfer
    ├── phase6.sh             # Phase 6: failover, resume, operator API, nonce sequencing
    └── phase7.sh             # Phase 7: 3 validators, threshold 2, sig-store, safety + recovery
```

## Architecture in one paragraph

`send()` locks an ERC-20 on the source gate and emits `Sent(submissionId, …)`. The
**validator** scans the source chain, independently recomputes the `submissionId`
(`bridge-core`, byte-identical to `BridgeHash.sol`), and — only on an exact match —
signs the EIP-191 digest and writes the signature to the file-backed store. The
**keeper** reads the store and, once ≥ threshold signatures exist, submits `claim()`
to the target gate, which re-derives the `submissionId`, verifies the signatures
against its validator set, guards against replay (`executed[]`), and releases funds.

## The sacred hash

`submissionId = keccak256(abi.encodePacked(SUBMISSION_PREFIX, debridgeId,
chainIdFrom, chainIdTo, amount, receiver, nonce))` (with an auto-params tail when an
execution payload is attached). It is defined once in `contracts/src/BridgeHash.sol`
and reproduced in `crates/bridge-core/src/lib.rs`. Phase 3 locks the two together:

```bash
cd contracts && forge test --match-contract GenFixtures   # Solidity writes fixtures
cd ..        && cargo test -p bridge-core                  # Rust must reproduce them
```

## Run the tests

```bash
# contracts (Phases 1–3): send, claim security suite, hash fixtures
cd contracts && forge test -vv

# cross-language hash equivalence (Phase 3)
cargo test -p bridge-core
```

## Run the end-to-end transfer (Phase 5)

Requires `forge`/`anvil`/`cast` (Foundry) and `cargo` on PATH.

```bash
bash scripts/e2e.sh
```

It starts two local chains (1337 @ :8545, 1338 @ :8546), deploys `Gate`+`TestToken`
on both, pre-funds the target gate and registers the asset, runs the Rust validator
and keeper, performs `send()` of 100 TST on 1337, and asserts the receiver is paid
100 TST on 1338 and that the replay guard (`executed[submissionId]`) is set. Logs
land in `.e2e-logs/`.

## Hardened validator (Phase 6)

The validator is now the real node, not a prototype:

- **Multi-RPC failover** (`provider::Failover`) — an ordered list of endpoints; every
  call tries the active one and rotates to the next on error. A `chainId` guard drops
  endpoints reporting the wrong network at startup.
- **Finality buffer** — only processes blocks up to `latest - block_confirmation`.
- **Resumable cursor** (`state::Runtime`) — `{last_block, nonces}` is persisted to a
  JSON state file (atomic temp-then-rename). On restart it resumes from `last_block + 1`
  without re-signing or skipping events.
- **Sequential-nonce enforcement** — per `chainIdTo`, a gap (`MISSED_NONCE`) or replay
  (`DUPLICATED_NONCE`) **pauses** the scanner instead of signing. An `submissionId`
  mismatch (lying RPC) also pauses. Decision logic is unit-tested (`cargo test -p validator`).
- **Operator API** (`api`, optional `[api]` block) — `GET /status`,
  `POST /pause`, `POST /resume`, `POST /rescan {"from_block":N}`.

Demonstrate every mechanism end-to-end:

```bash
bash scripts/phase6.sh
```

## N validators + threshold (Phase 7)

The trust model is now real: **multiple independent validators**, each with its own
key, all POSTing to the **`sig-store` HTTP service**; the keeper submits `claim()`
only once **≥ threshold distinct** signatures exist. The Gate enforces the threshold
on-chain (signatures sorted ascending by signer, deduped).

`sig-store` (axum) keeps the same `SubmissionRecord` shape as the file store and is
backed by a directory on disk:

```
GET  /health
POST /submissions        # upsert a record + signature; dedupe by signer
GET  /submissions        # all records (keeper polls)
GET  /submissions/:id    # one record
```

Validator/keeper pick the backend in `[store]`: `dir = "…"` (local file) **or**
`url = "http://sig-store:8080"` (HTTP). Demonstrate 3 validators / threshold 2,
the 1-of-3 safety case, and recovery:

```bash
bash scripts/phase7.sh
```

## Docker (Phase 7)

The off-chain stack is dockerized — `sig-store` + 3 validators + 1 keeper, plus two
anvil chains for local bring-up. Gate addresses in `docker/configs/*.toml` are
anvil's deterministic deploy addresses, so deployment is reproducible:

```bash
docker compose up -d anvil-src anvil-dst sig-store
bash docker/deploy.sh                                  # deploy + wire both chains
docker compose up -d validator1 validator2 validator3 keeper
```

> Not exercised in the WSL dev box used to build this (Docker Desktop WSL
> integration was off); `scripts/phase7.sh` covers the same topology with local
> processes.

## Config

`validator.toml` / `keeper.toml` are generated by the scripts. Shapes:

```toml
# validator.toml
[source]   chain_id, gate, start_block, block_confirmation, poll_interval_ms, max_block_range
           rpc = "http://…"                 # single endpoint (back-compat), OR
           rpcs = ["http://…", "http://…"]  # ordered failover list
           state_file = "validator-state.json"   # resumable cursor + nonce state
[signer]   # how this node holds its signing key — see "Key custody" below
[store]    dir = "…"   OR   url = "http://sig-store:8080"
[api]      bind = "127.0.0.1:9090"   # optional operator API

# keeper.toml
[target]   chain_id, rpc, gate, poll_interval_ms
[keeper]   # funded gas-payer key — same custody options as [signer]
[store]    dir = "…"   OR   url = "http://sig-store:8080"
```

### Key custody

No single key can move funds on-chain — `claim()` needs a threshold of *distinct*
validator signatures. That only holds if each relayer guards its own key well, so
`[signer]` (validator) and `[keeper]` (gas payer) both accept, in order of
preference — **exactly one** source:

```toml
[signer]
# 1. Encrypted keystore (Web3 Secret Storage / `cast wallet`) — recommended.
keystore = "/run/secrets/validator-keystore.json"
keystore_password_file = "/run/secrets/keystore-password"   # OR
keystore_password_env  = "KEYSTORE_PASSWORD"                # OR (dev) keystore_password = "…"

# 2. Raw key via env var — keeps the secret out of the file (Docker/systemd secret).
private_key_env = "VALIDATOR_PRIVATE_KEY"

# 3. Raw key inline — DEV ONLY (logged as a warning; a leaked config is a leaked key).
private_key = "0x…"
```

Setting more than one source (or a keystore without a password) is rejected at
startup. Secrets are redacted from any debug output of the config.

## What's next (per the build plan)

- **P8** asset registry + wrapped-token minting (`deployId`).
- **P9** testnet soak, chaos, audit.
```
