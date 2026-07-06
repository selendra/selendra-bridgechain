#!/usr/bin/env bash
# Build the solana-gate BPF program against the Solana 1.18.26 toolchain.
#
# Handles two cargo pitfalls when resolving against a current crates.io index:
#   * A modern rustup cargo writes a v4 Cargo.lock the 1.18 SBF toolchain can't
#     read — resolve with rustup cargo, then rewrite the lock to v3.
#   * A pile of crates in the dependency tree have jumped to `edition2024`
#     (needs rust 1.85+, newer than any Solana platform-tools). We pin each such
#     crate (and the roots that pull them — borsh's proc-macro-crate, blake3's
#     cc→jobserver→getrandom) back to its last pre-edition2024 release.
set -euo pipefail

SOLANA_BIN="$HOME/.local/share/solana/install/active_release/bin"
PT_BIN="$HOME/.cache/solana/v1.41/platform-tools/rust/bin"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/../programs/solana-gate"

RUSTUP_CARGO() { PATH="$HOME/.cargo/bin:$PATH" cargo "$@"; }

# 1. Resolve with the modern cargo, then pin every edition2024 crate away.
RUSTUP_CARGO generate-lockfile
pin() { RUSTUP_CARGO update -p "$1" --precise "$2" 2>/dev/null || true; }
pin borsh@1.7.0 1.5.0                   # 1.7 needs rustc 1.77; platform-tools has 1.75
pin blake3 1.5.1                       # -> digest 0.10 / block-buffer 0.10
pin zeroize_derive 1.4.2
pin proc-macro-crate@3.5.0 3.2.0       # -> toml_edit 0.22 (drops toml_parser/datetime 1.1)
pin indexmap@2.14.0 2.7.1              # -> hashbrown 0.15
pin jobserver 0.1.31                   # drops getrandom 0.3 -> wasip2 -> wit-bindgen

# Fail loudly if anything edition2024 still slipped through.
left="$(python3 "$SCRIPT_DIR/_detect_ed2024.py")"
if [ -n "$left" ]; then echo "ERROR: edition2024 crates remain:"; echo "$left"; exit 1; fi

# 2. Downgrade the lockfile format for the 1.18 SBF toolchain.
sed -i 's/^version = 4$/version = 3/' Cargo.lock

# 3. Build the .so. Here the host cargo must be rustup's (it delegates the
#    `cargo +solana build` step to the platform-tools toolchain); the v3 lock is
#    already pinned so no re-resolution to edition2024 versions happens.
export PATH="$SOLANA_BIN:$HOME/.cargo/bin:$PATH"
echo "host cargo: $(cargo --version)"
cargo-build-sbf

echo
echo "artifact:"
ls -la target/deploy/*.so
