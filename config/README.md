# JSON configs

Two files, two jobs. JSON carries no comments, so this is the field reference.

| file | drives | script |
| --- | --- | --- |
| [`deploy.config.json`](deploy.config.json) | deploying + wiring the contracts | `bash scripts/deploy-from-json.sh [config]` |
| [`bridge.config.json`](bridge.config.json) | running the mesh (validators, keeper, indexer, API) | `bash scripts/bridge-from-json.sh [config]` |

They are meant to be used in that order: the deploy script writes the addresses
it produced into `config/deployments/<name>.json` **and** patches them straight
into the runtime config named by `output.update_bridge_config`, so the second
step needs no copy-paste.

```bash
bash scripts/deploy-from-json.sh config/deploy.config.json     # deploy + wire
bash scripts/bridge-from-json.sh config/bridge.config.json     # run the mesh
bash scripts/bridge-from-json.sh config/bridge.config.json --status
bash scripts/bridge-from-json.sh config/bridge.config.json --stop
```

`scripts/run.sh` + `scripts/run.config` (bash) still work and are unchanged —
these two are the JSON-driven equivalent, with deployment and operation split so
you can redeploy without restarting, or restart without redeploying.

---

## `deploy.config.json`

`bash scripts/deploy-from-json.sh [config] [--dry-run] [--no-config-update]`

`--dry-run` validates everything and prints the plan without sending a
transaction. `--no-config-update` skips patching the runtime config.

| field | meaning |
| --- | --- |
| `name` | deployment label; names the output file and seeds a generated domain |
| `profile` | `"local"` or `"production"` — see the table below |
| `deployer.keystore` + `deployer.keystore_password_file` | encrypted Web3 keystore (preferred) |
| `deployer.private_key_env` | name of an env var holding the raw key |
| `deployer.private_key` | raw key inline — **dev only** |
| `gate.validators[]` | validator addresses baked into every gate. Duplicates are rejected: the Gate constructor dedupes, so `[A,B,B]` with threshold 2 would quietly ship a 2-of-2 gate |
| `gate.threshold` | signatures required to move funds |
| `gate.bridge_domain` | the mesh-generation binding, `0x` + 64 hex. Every gate in one generation shares it; a **new** deployment needs a **new** one, or the previous deployment's validator signatures replay against the fresh gates. `"auto"` derives one (local only) |
| `gate.guardian` | pause button, low trust, must differ from the owner (production) |
| `gate.owner` | multisig that receives ownership via two-step transfer (production) |
| `chains[]` | `chain_id`, `name`, `rpc_url`; `deploy_gate: false` + `gate` reuses an existing one. The RPC's reported chain id is verified before anything is sent |
| `assets[].symbol/name/decimals` | the bridgeable asset |
| `assets[].deployments[]` | `chain_id` + `address`; `"auto"` deploys a fresh `TestToken` (local only). An existing address must have contract code on that chain |
| `assets[].register_corridors` | wire the asset full-mesh with `setLocalToken` |
| `assets[].test_liquidity` | mint whole-token amounts to the deployer and the gate (local only) |
| `swap` | optional same-chain `SwapPool` for the Swap view (`deploy: true` is local only — `DeploySwap` mints unrestricted test tokens) |
| `solana` | the Solana leg — see below; `enabled: false` skips it entirely |
| `output.file` | where the produced addresses are written. Gitignored: the record repeats the RPC URLs it used, and a hosted endpoint's URL is a credential |
| `output.update_bridge_config` | runtime config to patch with gate/token/pool addresses and each chain's deploy block; `null` to skip |

### profiles

| | `local` | `production` |
| --- | --- | --- |
| gate deploy | `forge create` Gate impl + `GateProxy` | `script/DeployProd.s.sol`, which asserts every policy invariant on-chain |
| validators | ≥ 1 | ≥ 3 |
| threshold | ≥ 1 | strict majority (`> n/2`, and ≥ 2) |
| guardian / owner | optional | required; ownership goes to the multisig (two-step) |
| tokens | `"auto"` TestTokens allowed | real ERC-20 addresses only |
| test liquidity | allowed | refused |
| `bridge_domain` | `"auto"` allowed | must be pinned |
| corridors | sent by the deployer | **not sent** — after the handover only the owner may call `setLocalToken`, so the calldata is written to `governance_calls` in the output file for the multisig to execute |

After a production run the multisig must `acceptOwnership()` on every gate, then
execute `governance_calls`.

### the Solana leg

Solana is not an entry in `chains[]`. It is a different VM with a different
toolchain and a separate process, so it gets its own `solana` block — but it is
the *same bridge*: the program is initialized with the same validator set,
threshold and `bridge_domain` as the EVM gates, which is what makes a
submissionId computed on one side verify on the other.

| field | meaning |
| --- | --- |
| `chain_id` | deBridge's id for Solana (`7565164`). Not a Solana concept — it is the value hashed into every submissionId, and both sides must agree |
| `rpc` / `cluster` | endpoint and a label for the log line |
| `payer_keypair` | the fee payer. It signs `solana program deploy` and every governance instruction, **and it becomes the gate's owner** — `init` requires the program's upgrade authority, and there is no ownership-transfer instruction on this side, unlike the EVM gate's two-step handover. Whichever key deploys the program governs it |
| `gate_admin_bin` / `build` | the `gate-admin` client. It lives in the `solana-relayer` crate — its own cargo project, because `solana-client` pins `zeroize <1.4` and alloy needs `^1.5`, so no EVM-side crate can host it |
| `program.deploy` / `program.program_id` / `program.so_path` | deploy `solana_gate.so` (build it with `scripts/testing/build-solana.sh`) or reuse a deployed program |
| `program.use_rpc` | send the deploy's write transactions over JSON-RPC instead of the leader's TPU. Leave it on for hosted endpoints and containerised validators: the TPU path needs gossip reachability and otherwise stalls 20s and fails |
| `init.run` | initialize the gate if it is not already. Re-running is safe — the script reads the on-chain config first and leaves an initialized gate alone |
| `init.guardian` | pause-only key (may pause, not unpause), as on the EVM side |
| `init.max_validators` / `max_corridors` | the config account is sized for these at init and both vectors are refused growth past them, so it can never outgrow its buffer |
| `register_corridors` | register every EVM chain in `chains[]` as a destination. `send` refuses any `chain_id_to` governance has not registered; the instruction is idempotent |
| `assets[].mint` / `.vault` | the SPL mint and the program-owned vault. **Supplied, never created here** — the vault must be an SPL account for that mint, owned by the program's `vault_authority` PDA, with no delegate and no close authority (the program rejects anything else) |
| `assets[].from_chains` | which EVM chains this asset may arrive from (`"all"` or a list). One registration per source chain, exactly as the EVM side needs one `setLocalToken` per corridor — a claim commits only to the debridgeId, and that id differs per origin |
| `assets[].debridge_id` | for a Solana-NATIVE asset, the id it is bridged under. It is registered on the program *and* mapped on every EVM gate that carries the symbol. Leave `null` for an EVM-native asset |

The script refuses to touch a program whose on-chain `bridge_domain` differs from
this deployment's: that program belongs to an earlier generation, its domain is
immutable, and the only symptom would be transfers that never claim.


## `bridge.config.json`

`bash scripts/bridge-from-json.sh [config] [--generate-only|--stop|--status]`

Generates one TOML per process into `runtime.run_dir` and starts them.
`--generate-only` stops after writing the TOMLs, so you can inspect exactly what
each process is handed (or ship them to separate hosts, which is what a real
validator set looks like — one operator per key, not one machine running all of
them).

| field | meaning |
| --- | --- |
| `threshold` | signatures a claim needs; must match the deployed gates |
| `runtime.run_dir` | generated configs, logs, pid file, validator cursors |
| `runtime.bin_dir` | where the compiled services are (`target/debug`, `target/release`, …) |
| `runtime.build` | `cargo build` the services first |
| `database.url` | Postgres for the sig-store + indexer |
| `database.docker` | run that Postgres as a container. `--stop` removes the container but **keeps** the volume: it holds signatures, history and indexer cursors, and validators resume from file cursors rather than re-signing blocks they already scanned |
| `sig_store.tokens` | scoped credentials, one per role. With none set the store runs **unauthenticated** — signatures, claim status and the allowlist become writable by anything that can reach the port. `generate_if_unset` mints a random one per role per run into `run_dir/tokens.env` |
| `defaults` | per-chain fallbacks for `poll_interval_ms`, `max_block_range`, `start_block`, `block_confirmation`, `allow_zero_confirmation` |
| `chains[].rpcs` | ordered endpoints; validators fail over to the next on error |
| `chains[].gate` | the **proxy** address (never the implementation) |
| `chains[].source` / `.destination` | which roles this chain plays. Both `true` = full mesh, which is the normal case |
| `chains[].start_block` | scan floor. `0` re-scans a live chain's entire history; the deploy script sets each chain's deploy block for you |
| `chains[].block_confirmation` | finality buffer — **security critical**. Signing an event at the chain tip lets a reorg erase the deposit *after* the destination paid out. It must exceed the chain's reorg depth. `0` is refused unless `allow_zero_confirmation` is set, which is only safe on an instant-final dev chain (anvil) |
| `chains[].pool` / `.router` | SwapPool / SwapRouter to index, if any |
| `chains[].tokens[]` | symbol + address, served to the UI; `tokens[0]` is the chain's primary |
| `validators[]` | one entry per validator process: `name`, `signer` (same custody options as the deployer), `sources` (`"all"` or a chain-id list), optional operator `api` |
| `refund` | the two-phase refund attestation loop. Disabled ⇒ no validator votes on cancels and stranded transfers stay stranded — the safe default, since a node that cannot read the destination must not have an opinion on delivery. Its own `block_confirmation` guards the destination read |
| `keepers[]` | `name`, `signer`, `targets` (claims), `refund_sources` (refunds pay out where the funds were locked). Split into two keeper entries — one with only `targets`, one with only `refund_sources` — when you don't want both loops sharing an account's nonce |
| `solana` | the Solana relayers — see below; `enabled: false` skips them |
| `indexer` | history + refund eligibility sweep; the only writer of `refund_status`. EVM chains only — it speaks EVM JSON-RPC, so a transfer **delivered on Solana** is recorded as `stuck` / `refund_status: eligible` forever: the `Sent` is on an EVM chain it watches, the `Claimed` is not. Nothing acts on that nomination (an EVM validator never attests for a destination outside its `refund.destinations`, and the relayer re-reads the Solana gate before attesting), but the UI will show those transfers as stuck |
| `frontend` | the vite dev server for the UI. It reaches the API through vite's proxy (`VITE_PROXY_TARGET`), so the API needs no CORS and no public port. `node_bin` pins a toolchain when node is not on PATH — an nvm install usually isn't; leave it `null` to auto-detect the newest one |
| `graphql` | the read API the frontend talks to. It holds no database credential — it reads history through the sig-store on its reader token, because it is the only service meant to face the internet |


### the Solana relayers

`solana-relayer` is the Solana leg's validator, and it is a separate process for
a hard reason: `solana-client` and alloy cannot live in one binary. It signs
Solana-origin transfers into the same sig-store the EVM validators use, and
(when `deliver` is set) submits EVM→Solana claims on-chain.

| field | meaning |
| --- | --- |
| `program_id` | the deployed gate program; the deploy script fills it in |
| `commitment` | Solana's finality control — there is no block count here. `confirmed` and `processed` can both be rolled back by a fork, which is the same double-spend `block_confirmation` defends against on EVM, so anything but `finalized` is refused unless `allow_unfinalized` is set (a local test validator only) |
| `relayers[]` | one process per validator key. Each holds the **same secp256k1 key** that validator uses on the EVM side — one validator set attests for both VMs. Only `private_key` / `private_key_env` are supported here (no keystore) |
| `relayers[].deliver` | run the claim-submitting half. `payer_keypair` pays fees and rent and carries no bridge authority — the validator signatures do |
| `tokens[]` | symbol → mint, for the record and for the optional UI listing |
| `include_in_registry` | list Solana in the GraphQL registry the UI reads. Off by default: the API registers `rpc_url`+`gate` chains for on-chain `executed` lookups and speaks EVM JSON-RPC only, so a Solana row is listed, never polled |
| `bin` / `build` | the relayer binary (its own cargo project — see above) |

**Run at least `threshold` relayers, each with a distinct key.** Solana `Sent`
events are signed *only* by relayers — the EVM validators never scan Solana — so
with fewer, Solana-origin transfers stall below quorum and nothing says why. The
launcher refuses to start that configuration, and refuses two relayers sharing a
key (which would count one key twice toward the quorum).

### running several bridges

One config = one mesh. Every chain listed bridges to every other one in both
directions, so a third chain is one more entry in `chains[]` — nothing else
changes. To run *separate* meshes side by side (say staging and production, or
two disjoint validator sets), copy the file and give each its own `name`,
`runtime.run_dir`, `sig_store.bind`/`url`, `graphql.bind`, and
`database.docker.container`/`port`/`volume`. `--stop` matches only the processes
its own config started, so the two never tear each other down.

## keeping secrets out of the files

`signer` (validators, keepers) and `deployer` both accept, in order of
preference: `keystore` + `keystore_password_file`, `private_key_env`, or an
inline `private_key` (dev only — a leaked config is then a leaked key, and the
services log a warning at startup). The sig-store tokens accept the same
treatment via the environment. The shipped local configs use the well-known
public anvil keys on purpose: they are worthless, and they must never appear in
anything that touches a real network.
