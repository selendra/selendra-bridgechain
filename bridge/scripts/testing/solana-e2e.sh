#!/usr/bin/env bash
# Phase 8 — EVM <-> Solana bridge verification.
#
# There is no Solana runtime dependency here: this proves the cross-chain
# protocol end-to-end against the exact on-chain verification logic.
#
#   1. Foundry: Gate.sol accepts a 32-byte Solana receiver in send() (and still
#      rejects malformed widths) — the EVM->Solana source path.
#   2. bridge-solana: the Solana-side keccak submissionId is byte-identical to
#      Gate.sol / bridge-core across every shared fixture (incl. the Solana
#      32-byte-receiver and auto cases); secp256k1_recover accepts the same
#      validator signatures.
#   3. bridge-solana e2e: both directions, driven by real validator signatures —
#      EVM->Solana claim releases SPL (2-of-3, replay-blocked, below-threshold
#      refused); Solana->EVM send is scanned, recomputed, and its signatures pass
#      the EVM gate's verification rule.
#
# The deployable BPF program (crates/solana-gate) is a syscall-based
# reimplementation of the logic proven in step 3; building it needs the Solana
# toolchain (see that crate's README) and is intentionally out of scope here.
set -euo pipefail

export PATH="$HOME/.foundry/bin:$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."

echo "== [1/3] Foundry: EVM -> Solana send (32-byte receiver) =="
( cd contracts && forge test --match-contract "SolanaBridgeTest|SecurityTest|SendTest" )

echo
echo "== [2/3] bridge-solana: cross-chain hash + signature equivalence =="
cargo test -p bridge-solana --offline --test equivalence -- --nocapture

echo
echo "== [3/3] bridge-solana: both-direction end-to-end simulation =="
cargo test -p bridge-solana --offline --test e2e -- --nocapture

echo
echo "PASS: EVM <-> Solana bridge — hash locked across VMs, 2-of-3 threshold"
echo "      enforced both directions, replay guarded, funds released only on quorum."
