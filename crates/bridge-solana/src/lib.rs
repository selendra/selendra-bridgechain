//! bridge-solana — the Solana side of the EVM ↔ Solana bridge (Phase 8).
//!
//! The bridge protocol is chain-agnostic: the same sacred keccak submissionId
//! and the same EIP-191 secp256k1 validator signatures work on any VM that has a
//! keccak and a secp256k1-recover primitive. Solana has both (`keccak` and
//! `secp256k1_recover` syscalls), so a Solana gate program verifies the *exact*
//! signatures our existing EVM validators already produce — no new signing path.
//!
//! What lives here (all host-testable, alloy-free):
//!   * [`hash`]   — submissionId, byte-identical to `BridgeHash.sol`/`bridge_core`.
//!   * [`verify`] — recover EVM signers + threshold check (mirrors `_verifySignatures`).
//!   * [`gate`]   — the `SolanaGate` claim/send state machine (the program's brain).
//!   * [`instruction`] — the Borsh wire format a Solana program (de)serializes.
//!   * [`relayer`] — off-chain adapters: scan a `Sent` log, encode a `claim` ix.
//!
//! The deployable BPF program (../../crates/solana-gate) is a thin entrypoint
//! over [`gate`], built with `cargo build-sbf`.

pub mod gate;
pub mod hash;
pub mod instruction;
pub mod relayer;
pub mod verify;

/// deBridge's chain id for Solana mainnet — also the value used in the shared
/// cross-language hash fixtures. Used as `chainIdTo` for EVM→Solana and
/// `chainIdFrom` for Solana→EVM.
pub const SOLANA_CHAIN_ID: u64 = 7565164;
