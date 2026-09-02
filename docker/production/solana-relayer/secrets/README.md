Only needed in **deliver mode** — i.e. when your `configs/solana-relayer.toml`
has a `[target]` block. A sign-only relayer (the validator role) needs nothing
here.

## payer.json

The Solana keypair that pays fees and rent for `claim` transactions. It holds
**no bridge authority** — the validators' secp256k1 signatures carry that — so it
only ever needs enough SOL for fees. Keep the balance small and monitor it: an
empty payer means EVM→Solana transfers silently stop being delivered.

    solana-keygen new --outfile payer.json --no-bip39-passphrase
    solana address -k payer.json          # fund this
    solana balance -k payer.json

## Permissions

The relayer image runs as **uid 10001**, and docker compose bind-mounts
file-secrets with the host file's ownership while **ignoring** the
`uid`/`gid`/`mode` long syntax outside swarm mode. So a plain `chmod 600` as
your own user makes the keypair unreadable in the container.

Pick one shape; `../preflight.sh` accepts either and rejects everything else.

**A — strictest, needs root once:**

    sudo chown 10001:10001 payer.json
    chmod 600 payer.json

**B — no root needed.** The 0700 directory keeps other host users out; the file
itself must be world-readable so uid 10001 can read it through the bind mount,
which does not re-check directory traversal:

    chmod 700 .
    chmod 644 payer.json

## What is NOT here

The secp256k1 **validator signing key**, which lives in `../.env` as
`SOLANA_VALIDATOR_KEY`. That is not a choice — `solana-relayer`'s `[signer]`
accepts only `private_key` and `private_key_env`, with no encrypted-keystore
option, so it has no equivalent of the `validator/` and `keeper/` stacks'
`/run/secrets` keystore. Keep `../.env` at 0600 and treat this host accordingly.
