Put the validator's encrypted keystore and its password here. Both are mounted
read-only into the container as docker secrets under `/run/secrets/`.

## Create them

    cd secrets
    printf 'a-long-random-password' > keystore-password
    cast wallet import validator-key -k "$PWD" --interactive
    mv validator-key keystore.json

(`--interactive` prompts for the private key so it never reaches your shell
history. `--unsafe-password "$(cat keystore-password)"` avoids the second
prompt at the cost of putting the password in the process list.)

Expected files, neither ever committed (see the repo .gitignore):

    keystore.json        Web3 Secret Storage JSON
    keystore-password    the password (no trailing newline needed)

## Permissions — the part that bites

This image runs as **uid 10001**. Docker compose bind-mounts file-secrets with
the HOST file's ownership and mode, and it **ignores** the `uid`/`gid`/`mode`
long-syntax fields outside swarm mode. So the usual `chmod 600` as your own
user makes these unreadable in the container, and the only symptom is a restart
loop logging `Permission denied (os error 13)`.

Pick one shape. `../preflight.sh` accepts either and rejects everything else.

**A — strictest, needs root once.** Owned by the container user, unreadable by
any other host user:

    sudo chown 10001:10001 keystore.json keystore-password
    chmod 600 keystore.json keystore-password

**B — no root needed.** The 0700 directory is what keeps other host users out;
the files themselves must be world-readable so uid 10001 can read them through
the bind mount, which does not re-check directory traversal:

    chmod 700 .
    chmod 644 keystore.json keystore-password

## Do not

Never put a raw `private_key = "0x…"` in the TOML instead. The process logs a
warning when you do, and from then on a leaked config file is a leaked key.
