# solana-gate — on-chain Solana bridge program

The Solana counterpart of `contracts/src/Gate.sol`. Same protocol:

- `send` — lock SPL into the vault, bump the per-target nonce, emit a `Sent` event.
- `claim` — recompute the sacred `submissionId` (keccak syscall), verify a
  threshold of **distinct** validator signatures (`secp256k1_recover` syscall,
  same EIP-191 secp256k1 sigs the EVM gate accepts), block replay via an
  `["executed", submissionId]` PDA, and release SPL from the vault.
- `set_validator` / `set_threshold` — owner-gated governance.

## Why it isn't in the host workspace

Its `solana-program` / `spl-token` dependencies only compile for the Solana
BPF/SBF target, so it is `exclude`d from `bridge/Cargo.toml`. **The logic here is
not hypothetical:** it is a syscall-based reimplementation of the
[`bridge-solana`](../../crates/bridge-solana) crate, whose test suite proves the
hash and signature verification byte-for-byte against `Gate.sol` and
`bridge-core`. Treat `bridge-solana` as the tested reference and this as its
deployable form.

## Build & deploy (requires the Solana toolchain)

Use the helper — it pins the toolchain/dependency versions the 1.18 SBF compiler
needs (a current crates.io index otherwise pulls `edition2024` crates and a
lockfile format the toolchain can't read):

```bash
bash scripts/build-solana.sh          # -> target/deploy/solana_gate.so

# local cluster (Docker) + a real EVM->Solana claim, end to end:
docker run -d --name solana-node -p 8899:8899 -p 8900:8900 \
  solanalabs/solana:v1.18.26 solana-test-validator --ledger /tmp/ledger --quiet
bash scripts/solana-localnet-e2e.sh
```

`solana-localnet-e2e.sh` deploys the `.so`, `Init`s the Config PDA with the
validator set + threshold (the same EVM addresses the EVM gate trusts), pre-funds
the vault, and submits a `Claim` with two real validator signatures — asserting
on-chain release, replay rejection, and below-threshold refusal. **This has been
run and passes** against `solanalabs/solana:v1.18.26`.

> Verified with Solana 1.18.26 / platform-tools v1.41. The committed `Cargo.lock`
> captures the pinned dependency versions.

## Account layouts

| Instruction | Accounts (in order) |
|-------------|---------------------|
| `Send`  | config(w), payer(s), user_token(w), vault(w), spl_token_program |
| `Claim` | config, executed_pda(w), payer(s,w), vault(w), receiver_token(w), vault_authority, spl_token_program, system_program |

PDAs: `["config"]`, `["vault", mint]` authority, `["executed", submissionId]`.
