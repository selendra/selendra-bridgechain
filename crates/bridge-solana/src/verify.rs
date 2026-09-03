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

/// secp256k1 group order — the modulus the low-`s` rule is stated against.
const SECP256K1_N: [u8; 32] = [
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE,
    0xBA, 0xAE, 0xDC, 0xE6, 0xAF, 0x48, 0xA0, 0x3B, 0xBF, 0xD2, 0x5E, 0x8C, 0xD0, 0x36, 0x41, 0x41,
];

/// Is `s` above `n/2`? Both the Solana `secp256k1_recover` syscall and OpenZeppelin's
/// `ECDSA.recover` refuse such a signature; host-side recovery does not.
fn is_high_s(s: &[u8; 32]) -> bool {
    // n/2 is n >> 1; compare big-endian without a bignum dependency.
    let mut half = [0u8; 32];
    let mut carry = 0u8;
    for i in 0..32 {
        let byte = SECP256K1_N[i];
        half[i] = (byte >> 1) | (carry << 7);
        carry = byte & 1;
    }
    *s > half
}

/// Re-encode `sig65` in the only form the Solana gate (and the EVM one) accepts:
/// low-`s`, with `v ∈ {27, 28}`.
///
/// ## Why relayers need this
///
/// `s` and `n - s` are both valid signatures from the same key over the same
/// message, and host-side recovery accepts either — `k256::Signature::from_slice`
/// checks for zero scalars, not for canonicality. The on-chain verifiers do not:
/// `secp256k1_recover` refuses a high-`s` signature, and so does OpenZeppelin's
/// `ECDSA.recover` on the EVM side.
///
/// A relayer that forwards the bytes it was handed therefore builds an instruction
/// that fails wholesale on ONE bad entry, with the off-chain quorum still reading
/// as satisfied — so the retry loop resubmits the same doomed instruction forever.
/// One validator, or one honest validator whose ECDSA library does not normalise,
/// could freeze every claim, cancel and refund that way.
///
/// The store canonicalises on the way in now, so this is the heal for rows written
/// before that and for any store restored from an older backup. Returns `None` for
/// bytes that are not a 65-byte signature at all.
pub fn canonical_signature(sig65: &[u8]) -> Option<[u8; 65]> {
    if sig65.len() != 65 {
        return None;
    }
    let mut out = [0u8; 65];
    out.copy_from_slice(sig65);

    // Accept `v` in the two encodings that recover correctly, normalise to 27/28.
    out[64] = match out[64] {
        0 | 27 => 27,
        1 | 28 => 28,
        _ => return None,
    };

    let mut s = [0u8; 32];
    s.copy_from_slice(&out[32..64]);
    if is_high_s(&s) {
        // s := n - s, and flip the parity so the recovery id still selects the
        // same public key.
        let mut borrow = 0i16;
        let mut low = [0u8; 32];
        for i in (0..32).rev() {
            let diff = SECP256K1_N[i] as i16 - s[i] as i16 - borrow;
            if diff < 0 {
                low[i] = (diff + 256) as u8;
                borrow = 1;
            } else {
                low[i] = diff as u8;
                borrow = 0;
            }
        }
        out[32..64].copy_from_slice(&low);
        out[64] = if out[64] == 27 { 28 } else { 27 };
    }
    Some(out)
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


#[cfg(all(test, feature = "recover"))]
mod canonical_tests {
    use super::*;

    fn n_minus(s: &[u8; 32]) -> [u8; 32] {
        let mut out = [0u8; 32];
        let mut borrow = 0i16;
        for i in (0..32).rev() {
            let d = SECP256K1_N[i] as i16 - s[i] as i16 - borrow;
            if d < 0 {
                out[i] = (d + 256) as u8;
                borrow = 1;
            } else {
                out[i] = d as u8;
                borrow = 0;
            }
        }
        out
    }

    /// A real signature, its malleated twin, and its `v ∈ {0,1}` variant must all
    /// normalise to the same bytes — and to a form `recover_evm_address` accepts.
    #[test]
    fn every_accepted_encoding_normalises_to_the_same_bytes() {
        use k256::ecdsa::{signature::hazmat::PrehashSigner, SigningKey};

        let sk = SigningKey::from_bytes(&[7u8; 32].into()).unwrap();
        let digest = eth_signed_digest(&[0x42u8; 32]);
        let (sig, recid): (Signature, RecoveryId) = sk.sign_prehash(&digest).unwrap();

        let mut low = [0u8; 65];
        low[..64].copy_from_slice(&sig.to_bytes());
        low[64] = 27 + recid.to_byte();
        assert_eq!(canonical_signature(&low).unwrap(), low, "already canonical");

        // v in {0,1}
        let mut v01 = low;
        v01[64] -= 27;
        assert_eq!(canonical_signature(&v01).unwrap(), low);

        // high-s twin
        let mut s = [0u8; 32];
        s.copy_from_slice(&low[32..64]);
        assert!(!is_high_s(&s), "k256 signs low-s");
        let mut high = low;
        high[32..64].copy_from_slice(&n_minus(&s));
        high[64] = if low[64] == 27 { 28 } else { 27 };
        let mut hs = [0u8; 32];
        hs.copy_from_slice(&high[32..64]);
        assert!(is_high_s(&hs), "premise: the twin is high-s");

        assert_eq!(
            canonical_signature(&high).unwrap(),
            low,
            "the malleated twin must normalise back to the signer's own form"
        );
    }

    /// Normalising must never change WHO signed.
    #[test]
    fn normalising_preserves_the_signer() {
        use k256::ecdsa::{signature::hazmat::PrehashSigner, SigningKey};

        let sk = SigningKey::from_bytes(&[9u8; 32].into()).unwrap();
        let digest = eth_signed_digest(&[0xABu8; 32]);
        let (sig, recid): (Signature, RecoveryId) = sk.sign_prehash(&digest).unwrap();
        let mut low = [0u8; 65];
        low[..64].copy_from_slice(&sig.to_bytes());
        low[64] = 27 + recid.to_byte();

        let mut s = [0u8; 32];
        s.copy_from_slice(&low[32..64]);
        let mut high = low;
        high[32..64].copy_from_slice(&n_minus(&s));
        high[64] = if low[64] == 27 { 28 } else { 27 };

        let expected = recover_evm_address(&digest, &low).unwrap();
        let healed = canonical_signature(&high).unwrap();
        assert_eq!(recover_evm_address(&digest, &healed).unwrap(), expected);
    }

    #[test]
    fn malformed_input_has_no_canonical_form() {
        assert!(canonical_signature(&[0u8; 64]).is_none(), "wrong length");
        let mut bad_v = [1u8; 65];
        bad_v[64] = 99;
        assert!(canonical_signature(&bad_v).is_none(), "impossible v");
    }

    #[test]
    fn half_order_is_computed_correctly() {
        // n/2 = 0x7FFFFFFF...5D576E7357A4501DDFE92F46681B20A0
        let mut just_above = [0u8; 32];
        just_above[..16].copy_from_slice(&[
            0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0xFF, 0xFF,
        ]);
        just_above[16..].copy_from_slice(&[
            0x5D, 0x57, 0x6E, 0x73, 0x57, 0xA4, 0x50, 0x1D, 0xDF, 0xE9, 0x2F, 0x46, 0x68, 0x1B,
            0x20, 0xA1,
        ]);
        assert!(is_high_s(&just_above), "one above n/2 is high");

        let mut exactly = just_above;
        exactly[31] = 0xA0;
        assert!(!is_high_s(&exactly), "exactly n/2 is not high");
    }
}
