//! Validator-signature verification — the Solana-side reproduction of
//! `Gate.sol::_verifySignatures`.
//!
//! Validators sign the EIP-191 `eth_sign` digest of the raw 32-byte
//! submissionId with secp256k1. The *same* signatures the keeper submits to the
//! EVM gate are verified here: a Solana gate program calls `secp256k1_recover`
//! + `keccak` to recover each signer's 20-byte Ethereum address, exactly as we
//! do below with `k256`. So one validator set, one key each, signs for both VMs.

#[cfg(feature = "recover")]
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};

use crate::hash::keccak;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerifyError {
    #[error("signature is not 65 bytes")]
    BadLength,
    #[error("recovery id (v) must be 27 or 28, got {0}")]
    BadRecoveryId(u8),
    #[error("could not recover a signer from the signature")]
    Unrecoverable,
    #[error("signatures must be sorted by signer, strictly ascending (dup or unordered)")]
    InvalidSignerOrder,
    #[error("not enough validator signatures: got {got}, need {want}")]
    NotEnoughSignatures { got: u32, want: u32 },
}

/// EIP-191 `eth_sign` digest: `keccak256("\x19Ethereum Signed Message:\n32" || id)`.
pub fn eth_signed_digest(submission_id: &[u8; 32]) -> [u8; 32] {
    let mut p = Vec::with_capacity(28 + 32);
    p.extend_from_slice(b"\x19Ethereum Signed Message:\n32");
    p.extend_from_slice(submission_id);
    keccak(&p)
}

/// Recover the 20-byte Ethereum address that produced `sig65` over `digest`.
///
/// `sig65` is `r(32) || s(32) || v(1)` with `v ∈ {27, 28}` (OZ ECDSA form) — the
/// exact bytes the validator wrote to the store. On Solana this is
/// `secp256k1_recover(digest, v-27, r||s)` then `keccak(pubkey)[12..]`.
#[cfg(feature = "recover")]
pub fn recover_evm_address(digest: &[u8; 32], sig65: &[u8]) -> Result<[u8; 20], VerifyError> {
    if sig65.len() != 65 {
        return Err(VerifyError::BadLength);
    }
    let v = sig65[64];
    if v != 27 && v != 28 {
        return Err(VerifyError::BadRecoveryId(v));
    }
    let recid = RecoveryId::from_byte(v - 27).ok_or(VerifyError::Unrecoverable)?;
    let sig = Signature::from_slice(&sig65[..64]).map_err(|_| VerifyError::Unrecoverable)?;
    let vk = VerifyingKey::recover_from_prehash(digest, &sig, recid)
        .map_err(|_| VerifyError::Unrecoverable)?;

    // address = keccak256(uncompressed_pubkey[1..65])[12..32]
    let point = vk.to_encoded_point(false);
    let pubkey = point.as_bytes(); // 65 bytes: 0x04 || X || Y
    let h = keccak(&pubkey[1..]);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&h[12..]);
    Ok(addr)
}

/// Verify a threshold of distinct validator signatures over `submission_id`.
///
/// Mirrors `Gate.sol::_verifySignatures` byte-for-byte: signatures MUST be sorted
/// by recovered signer address strictly ascending (which both de-duplicates and
/// bounds work), and at least `threshold` of them must be known validators.
/// Returns the count of valid validator signatures on success.
#[cfg(feature = "recover")]
pub fn verify_threshold(
    submission_id: &[u8; 32],
    signatures: &[Vec<u8>],
    is_validator: impl Fn(&[u8; 20]) -> bool,
    threshold: u32,
) -> Result<u32, VerifyError> {
    let digest = eth_signed_digest(submission_id);
    // Seeded at the zero address and compared on EVERY signature, exactly as
    // `Gate.sol` does (`address last = address(0)`). An earlier version skipped
    // the comparison for the first signature, which let a recovery to the zero
    // address through where Solidity rejects it — a divergence in two
    // implementations whose whole contract is to be byte-for-byte equivalent.
    let mut last = [0u8; 20];
    let mut count: u32 = 0;
    for sig in signatures {
        let signer = recover_evm_address(&digest, sig)?;
        // strictly ascending => distinct signers, no duplicates
        if signer <= last {
            return Err(VerifyError::InvalidSignerOrder);
        }
        if is_validator(&signer) {
            count += 1;
        }
        last = signer;
    }
    if count < threshold {
        return Err(VerifyError::NotEnoughSignatures { got: count, want: threshold });
    }
    Ok(count)
}
