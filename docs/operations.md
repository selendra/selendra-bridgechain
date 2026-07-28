# Operations

How to run the stack, configure a node, hold the keys, and deploy.

This document is written from the code.
Where it and the sources disagree, the sources are right and this is a bug.
Read [`architecture.md`](./architecture.md) first if you have not; this one assumes you know what the processes do.

---

## 1. The processes

Eight crates, five of them binaries.

| Binary | What it needs | What breaks without it |
| --- | --- | --- |
| `sig-store` | Postgres (or a directory) | Nothing works. It is the bulletin board every other process reads. |
| `validator` | source RPC, a signing key, the store | No transfer is ever attested. One per independent operator. |
| `keeper` | target RPC, a funded key, the store | Nothing is ever submitted on-chain. Anyone can run one; it is permissionless. |
| `indexer` | Postgres, RPC per chain | History, stuck detection, and **the entire refund lifecycle**. It is the only writer of `refund_status`. |
| `graphql-api` | the store, optionally Postgres | The frontend has no backend. |

The dependency that catches people out is the refund one.
A refund needs the indexer running, because the keeper's refund loop only ever sees candidates the store has already nominated, and the sweep that nominates them (`bridge_db::Db::sweep_refund_eligible`) is called from the indexer and nowhere else.

> **Known gap.** `Dockerfile` builds only `validator`, `keeper`, and `sig-store`, and `docker-compose.yml` deploys only those.
> In the stack this repository currently ships, refunds never advance and the frontend has no backend.
> Tracked as H2 in `report.md`.
> Until that is fixed, run `indexer` and `graphql-api` yourself against the compose Postgres.

---

## 2. Running it locally

### 2.1 The scripted way

`scripts/testing/` holds the end-to-end harnesses.
They boot their own anvil chains, deploy, wire, run the Rust processes, and assert an outcome.
Each resolves its own root, so they run from anywhere.

They need Foundry (`forge`, `anvil`, `cast`) and `cargo` on `PATH`.

```bash
bash scripts/testing/e2e.sh          # one transfer across two chains, end to end
bash scripts/testing/phase6.sh       # failover, resume, operator API, nonce sequencing
bash scripts/testing/phase7.sh       # 3 validators, threshold 2, sig-store, recovery
bash scripts/testing/refund-e2e.sh   # the two-phase cancel-then-refund protocol
bash scripts/testing/db-e2e.sh       # allowlists and history against Postgres
```

Start with `e2e.sh`.
If it passes, the toolchain is sound and the rest will run.

Two scripts do not currently work, for reasons unrelated to paths.
`build-solana.sh` calls `scripts/testing/_detect_ed2024.py`, and `solana-localnet-e2e.sh` needs `tools/localnet/`.
Both were deleted in commit `525b109` and neither has been restored.

### 2.2 The compose way

```bash
docker compose up -d anvil-src anvil-dst postgres sig-store
bash docker/deploy.sh                                   # deploy + wire both chains
docker compose up -d validator1 validator2 validator3 keeper
```

`docker/deploy.sh` deploys `TestToken` then `Gate` from anvil account 0 on a fresh chain, so the addresses are deterministic and already baked into `docker/configs/*.toml`.
It asserts that the deployed addresses match the baked ones and refuses to continue if they do not, which is the check that catches a non-fresh chain.

Then send a transfer:

```bash
cast send 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
  'send(address,uint256,uint256,bytes,bytes)' \
  0x5FbDB2315678afecb367f032d93F642f64180aa3 100000000000000000000 1338 \
  0x976EA74026E726554dB657fA54763abd0C3a0aa9 0x \
  --rpc-url http://127.0.0.1:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
```

Override the shared sig-store secret, which defaults to `dev-local-bridge-token`:

```bash
SIG_STORE_TOKEN=$(openssl rand -hex 32) docker compose up -d
```

### 2.3 Frontend

```bash
bash scripts/testing/run-dev.sh      # graphql-api + vite, detached
```

In dev, `vite.config.ts` proxies `/graphql` and `/health` to the API.
In production the frontend must be served same-origin with the API, or built with `VITE_API` pointing at it.
There is no production serving story in compose.

---

## 3. Configuring a validator

Reference: `crates/validator/src/config.rs`.
Working examples: `docker/configs/val1.toml`.

```toml
[[sources]]                        # repeatable; one block per chain to watch
chain_id         = 1337
rpcs             = ["http://rpc-a", "http://rpc-b"]   # ordered failover list
gate             = "0x…"
start_block      = 0
block_confirmation = 12            # SEE BELOW
poll_interval_ms = 1000
max_block_range  = 1000
state_file       = "/data/val1-state.json"

[signer]                           # see section 4
keystore = "/run/secrets/validator-keystore.json"
keystore_password_file = "/run/secrets/keystore-password"

[store]
url = "http://sig-store:8080"      # or dir = "…" for a local file store

[api]                              # optional operator API
bind  = "127.0.0.1:9090"
token = "…"                        # or the VALIDATOR_API_TOKEN env var

[refund]                           # optional; omit and this node never attests refunds
timeout_secs     = 3600
poll_interval_ms = 15000
block_confirmation = 64            # SEE BELOW
[[refund.destinations]]
chain_id = 1338
rpcs     = ["http://rpc-dst"]
gate     = "0x…"
```

What `Config::load` rejects at startup, so you find out immediately rather than in production:

- no `[[sources]]` at all
- two sources with the same `chain_id`, or sharing a `state_file` (two scan loops would clobber one cursor)
- a `[refund]` block with no destinations, or duplicate destination chain ids
- `[refund] block_confirmation = 0` without `allow_zero_confirmation = true`
- a `[refund]` block with a file-backed `[store]`, because the unclaimed-timeout gate lives in the DB-backed sweep and the file store does not have one

### 3.1 The two finality buffers, and which one is actually enforced

There are two `block_confirmation` settings and they are not equally protected.

**`[refund] block_confirmation`** is enforced.
`Config::load` refuses to start at `0` unless you set `allow_zero_confirmation = true`, and the reason is spelled out in the source: a refund on the source chain is irreversible and is authorised solely on having read `cancelled == true` on the destination.
If that read is at the chain tip and the destination later reorgs the cancel away, the original claim signatures become live again, and the transfer is paid on the destination *and* refunded on the source.
Set it above the destination chain's maximum reorg depth.

**`[[sources]] block_confirmation`** is **not** enforced.
It defaults to `0`, nothing validates it, and `SourceChain` has no `allow_zero_confirmation` field at all.
The shipped `docker/configs/*.toml` set one anyway, under `[source]`, with a comment warning never to set it on a real chain.
Serde discards it in silence, so that line is decoration.
The `[refund]` blocks in the same files set the same key where it *is* a real field and *is* honoured, which is exactly why the discarded one is easy to miss.

Until `#[serde(deny_unknown_fields)]` lands (M4 in `report.md`), treat the source-chain buffer as unguarded and set it explicitly per chain.

### 3.2 Operator API

```
GET  /status
POST /pause
POST /resume
POST /rescan {"from_block": N}
```

The scanner pauses itself on a nonce gap, a nonce replay, or a `submissionId` mismatch, which is the signal that an RPC is lying or that events were missed.
A pause is a real safety stop and needs a human to look before `/resume`.

Note that `paused` is runtime-only state.
`state::load_or_init` hardcodes `paused: false`, so a validator that paused on an anomaly and was then restarted comes back up unpaused, having forgotten the anomaly.
Do not restart a paused validator as a way of clearing the condition (M2 in `report.md`).

---

## 4. Key custody

Reference: `crates/bridge-core/src/signer.rs`.
The same `[signer]` / `[keeper]` shape applies to both node types.

The bridge's safety rests on validators holding *distinct, well-guarded* keys.
The Gate needs a threshold of them, so no single key is ever enough, but that only holds if each operator guards its own.
A raw key in a TOML turns "leak the config" into "leak the key".

Exactly one key source must be set.
Both zero and more than one fail loudly at startup.

```toml
[signer]
# 1. Encrypted keystore (Web3 Secret Storage / `cast wallet`). Recommended.
keystore = "/run/secrets/validator-keystore.json"
keystore_password_file = "/run/secrets/keystore-password"   # OR
keystore_password_env  = "KEYSTORE_PASSWORD"                # OR (dev) keystore_password = "…"

# 2. Raw key via env var. Keeps the secret out of the file (Docker/systemd secret).
private_key_env = "VALIDATOR_PRIVATE_KEY"

# 3. Raw key inline. DEV ONLY; logged as a warning at startup.
private_key = "0x…"
```

Secrets are redacted from the config's debug output.

Other secrets in the system:

| Secret | Where it comes from | Notes |
| --- | --- | --- |
| sig-store bearer token | `SIG_STORE_TOKEN` | Unset means the API is unauthenticated, and the process warns about it. Compose defaults to `dev-local-bridge-token`. |
| validator operator API token | `[api] token` or `VALIDATOR_API_TOKEN` | Unset means `/pause`, `/resume`, and `/rescan` are unauthenticated. |
| Postgres URL | `DATABASE_URL` or config | |

`docker/configs/*.toml` carry inline private keys.
They are anvil's well-known development keys, so nothing there is at risk today, and `.dockerignore` excludes them from the Docker build context.
The pattern is still the one to break before a real key goes anywhere near it.

---

## 5. Deploy checklist

There is no reviewed production deploy script in this repository.
`contracts/script/DeploySwap.s.sol` and `DeployXSwap.s.sol` are local demos: threshold-1 gates and unrestricted-mint tokens, with no `block.chainid` guard to stop them running against a real network (L13 in `report.md`).
`docker/deploy.sh` is closer to right but is anvil-specific.

Until a real script exists, this is the checklist it would need to encode.
Every line is an assertion to make *after* deploying, not a step to trust.

**Gate, per chain**

1. `owner` is the intended cold key, and `pendingOwner` is zero.
   Ownership transfer is two-step; an unaccepted transfer leaves the old owner in control.
2. `isValidator[v]` is true for exactly the intended set, and false for everything else.
   Enumerate it; do not assume the constructor argument was right.
3. `threshold` is the intended value and is greater than 1.
   A threshold of 1 means a single signature releases funds.
4. **`guardian` is set and is not `address(0)`.**
   `setGuardian` is called nowhere in this repository, so it is zero in every deployment this repo can currently produce (M8).
   The guardian can pause but never unpause and never move funds, so it is safe to hold hot.
   Without it, the only key that can stop the bridge in an incident is the owner key, which is the one you most want to keep cold.
5. `paused` is false.
6. `localToken[debridgeId]` is set for every asset the gate must pay out, and the gate holds liquidity in each.
   A transfer to an unregistered asset cannot be claimed.

**SwapPool, per chain, if deployed**

7. `oracle` is the intended key and is separate from `owner`.
8. `maxPriceDeviationBps` is set, and you understand that it is a per-call cap with no time gate, so N calls in one block walk the price N times (M5).
9. `stable` is the intended token and its price is `PRICE_ONE`.

**Off-chain**

10. Each validator has a distinct key, and no two validators share a `state_file`.
    The compose stack mounts one `val-state` volume across all three; the paths inside it differ, which is safe today and one typo away from two validators sharing a nonce cursor.
11. `block_confirmation` is set explicitly on every source and every refund destination, above that chain's reorg depth.
    Remember the source-chain one is unvalidated.
12. `SIG_STORE_TOKEN` is a real secret, not the compose default.
13. `indexer` is running, or refunds silently do not exist.
14. `refund_timeout_secs` on the indexer matches `[refund] timeout_secs` on the validators.
    The indexer's value is the one that gates anything; the validator's is advisory and exists so the intended window is visible in one place.

**Verify the deploy end to end before announcing it.**
Send a dust transfer through the corridor and watch it claim.
Then strand one deliberately, on an unregistered asset, and watch it cancel and refund.
The refund path is the one that is easiest to ship broken, because nothing about a working transfer exercises it.

---

## 6. Incident response

**Stop the bleeding.**
`pause()` on the Gate, callable by owner or guardian.
`unpause()` is owner-only, deliberately, so a compromised guardian can cause a denial of service and nothing worse.

Know what pausing stops.
`whenNotPaused` guards `send`, `claim`, `cancel`, **and** `refund`, so a pause freezes recovery as well as traffic.
Transfers already in flight sit locked on the source chain with no path out until you unpause.
That is the correct trade in an active incident, and it is not a state to leave the bridge in while you investigate at leisure.

**Stop a validator signing.**
`POST /pause` on its operator API, or stop the process.
Signatures already in the store stay there; pausing prevents new ones.

**A validator paused itself.**
Read the log for `MISSED_NONCE`, `DUPLICATED_NONCE`, or a `submissionId` mismatch.
All three mean an RPC is lying or events were missed.
Find out which before resuming, and use `POST /rescan {"from_block": N}` rather than restarting the process, because a restart clears the pause flag without clearing the cause.

**A transfer is stuck.**
Check `status` in the DB.
Note that a `claim()` that reverted on-chain is currently recorded as `claimed` anyway, which permanently excludes it from the refund sweep (H1).
Until that is fixed, a stuck transfer whose status says `claimed` may need the row corrected by hand before the refund path will see it.
