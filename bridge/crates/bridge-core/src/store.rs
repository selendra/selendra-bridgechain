//! A dead-simple, file-backed signature store: one JSON file per submissionId.
//!
//! This is the local-dev stand-in for the deBridge API / Arweave. The validator
//! upserts its signature into `<dir>/<submissionId>.json`; the keeper reads the
//! directory, and once a record has >= threshold signatures, submits `claim()`.
//! Phase 7 replaces this with the HTTP `sig-store` service (same record shape).
//!
//! ## Trust boundary (security)
//!
//! In Phase 7 this store is fronted by an **unauthenticated** HTTP service, so
//! `upsert_signature` is a trust boundary: callers are untrusted. It therefore
//! enforces three invariants (the `abi` feature is required for the cryptographic
//! ones — every real caller, validator/keeper/sig-store, enables it):
//!
//!   1. **id ⇄ params binding** — `submission_id` MUST equal the canonical
//!      `keccak256` of the record's own parameters. An attacker cannot pin a real
//!      submissionId onto forged params (and vice-versa).
//!   2. **immutable params** — once a submissionId is stored, its parameters can
//!      never be changed; only new signatures may be merged in. This stops a
//!      poisoning attack that overwrites a legitimate record's `amount`/`receiver`
//!      while keeping the genuine validator signatures.
//!   3. **authentic signatures** — each signature must actually recover to its
//!      claimed `signer` over the EIP-191 digest of the submissionId, so junk
//!      signatures can't inflate the count past the threshold.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One validator's signature over a submissionId.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignerSig {
    /// Recovered/known signer address, `0x`-prefixed.
    pub signer: String,
    /// 65-byte ECDSA signature (r||s||v, v in {27,28}), `0x`-prefixed.
    pub signature: String,
}

/// Everything the keeper needs to rebuild and submit a `claim()`, plus the
/// collected validator signatures. Numeric fields are stringly-typed to avoid
/// precision loss for `amount` (uint256).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmissionRecord {
    pub submission_id: String,
    pub debridge_id: String,
    /// decimal string (uint256)
    pub amount: String,
    pub chain_id_from: u64,
    pub chain_id_to: u64,
    pub nonce: u64,
    /// `0x`-prefixed hex; raw bytes of the receiver
    pub receiver: String,
    /// `0x`-prefixed hex; `0x` (empty) when there is no execution payload
    pub auto_params: String,
    /// `0x`-prefixed hex; packed source sender
    pub native_sender: String,
    pub signatures: Vec<SignerSig>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("malformed field: {0}")]
    BadField(&'static str),
    #[error("submission_id {claimed} does not match the recomputed id {computed} for these params")]
    IdMismatch { claimed: String, computed: String },
    #[error("params conflict: submissionId {0} is already stored with different parameters (params are immutable)")]
    ParamsConflict(String),
    #[error("signature is not recoverable / invalid")]
    BadSignature,
    #[error("signature recovers to {recovered}, not the claimed signer {claimed}")]
    SignerMismatch { claimed: String, recovered: String },
}

fn file_path(dir: &Path, submission_id: &str) -> PathBuf {
    let id = submission_id.strip_prefix("0x").unwrap_or(submission_id);
    dir.join(format!("{id}.json"))
}

/// Ensure the store directory exists.
pub fn ensure_dir(dir: &Path) -> Result<(), StoreError> {
    std::fs::create_dir_all(dir)?;
    Ok(())
}

/// True iff two records carry identical transfer parameters (everything but the
/// collected signatures). Hex/hash fields are compared case-insensitively.
fn same_params(a: &SubmissionRecord, b: &SubmissionRecord) -> bool {
    a.submission_id.eq_ignore_ascii_case(&b.submission_id)
        && a.debridge_id.eq_ignore_ascii_case(&b.debridge_id)
        && a.amount == b.amount
        && a.chain_id_from == b.chain_id_from
        && a.chain_id_to == b.chain_id_to
        && a.nonce == b.nonce
        && a.receiver.eq_ignore_ascii_case(&b.receiver)
        && a.auto_params.eq_ignore_ascii_case(&b.auto_params)
        && a.native_sender.eq_ignore_ascii_case(&b.native_sender)
}

#[cfg(feature = "abi")]
fn hex_bytes(field: &'static str, s: &str) -> Result<Vec<u8>, StoreError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.is_empty() {
        return Ok(Vec::new());
    }
    hex::decode(s).map_err(|_| StoreError::BadField(field))
}

/// Recompute the canonical `submissionId` from a record's parameters, exactly as
/// the Gate contract would on `claim()`. This is THE check that binds an id to its
/// params: if the record's `submission_id` doesn't equal this, the record is forged.
#[cfg(feature = "abi")]
pub fn canonical_submission_id(rec: &SubmissionRecord) -> Result<alloy_primitives::B256, StoreError> {
    use alloy::sol_types::SolValue;
    use alloy_primitives::{B256, U256};
    use std::str::FromStr;

    let debridge_id = B256::from_str(&rec.debridge_id).map_err(|_| StoreError::BadField("debridge_id"))?;
    let amount = U256::from_str(&rec.amount).map_err(|_| StoreError::BadField("amount"))?;
    let receiver = hex_bytes("receiver", &rec.receiver)?;
    let auto_params = hex_bytes("auto_params", &rec.auto_params)?;
    let native_sender = hex_bytes("native_sender", &rec.native_sender)?;
    let chain_from = U256::from(rec.chain_id_from);
    let chain_to = U256::from(rec.chain_id_to);
    let nonce = U256::from(rec.nonce);

    let id = if auto_params.is_empty() {
        crate::submission_id(debridge_id, amount, chain_from, chain_to, nonce, &receiver)
    } else {
        let ap = crate::abi::AutoParamsTo::abi_decode(&auto_params)
            .map_err(|_| StoreError::BadField("auto_params"))?;
        let auto = crate::AutoParams {
            execution_fee: ap.executionFee,
            flags: ap.flags,
            fallback_address: ap.fallbackAddress.to_vec(),
            data: ap.data.to_vec(),
            native_sender,
        };
        crate::submission_id_with_auto(debridge_id, amount, chain_from, chain_to, nonce, &receiver, &auto)
    };
    Ok(id)
}

/// Verify a signature genuinely recovers to its claimed signer over the EIP-191
/// digest of `submission_id` (the same digest the validator signs and the Gate
/// verifies). Returns the recovered address on success.
#[cfg(feature = "abi")]
pub fn verify_signature(
    submission_id: alloy_primitives::B256,
    sig: &SignerSig,
) -> Result<alloy_primitives::Address, StoreError> {
    use alloy_primitives::{Address, Signature};
    use std::str::FromStr;

    let claimed = Address::from_str(&sig.signer).map_err(|_| StoreError::BadField("signer"))?;
    let raw = hex_bytes("signature", &sig.signature)?;
    let signature = Signature::try_from(raw.as_slice()).map_err(|_| StoreError::BadSignature)?;
    let recovered = signature
        .recover_address_from_msg(submission_id.as_slice())
        .map_err(|_| StoreError::BadSignature)?;
    if recovered != claimed {
        return Err(StoreError::SignerMismatch {
            claimed: format!("{claimed:#x}"),
            recovered: format!("{recovered:#x}"),
        });
    }
    Ok(recovered)
}

/// Insert or update a record, merging in `sig` (deduped by signer, case-insensitive).
/// Returns the resulting record (with all known signatures).
///
/// This is an untrusted-input trust boundary — see the module docs. With the `abi`
/// feature it rejects (1) records whose `submission_id` doesn't match their params,
/// (2) attempts to change the params of an already-stored submissionId, and (3)
/// signatures that don't recover to their claimed signer.
pub fn upsert_signature(
    dir: &Path,
    mut record: SubmissionRecord,
    sig: SignerSig,
) -> Result<SubmissionRecord, StoreError> {
    ensure_dir(dir)?;

    // (1) Bind id <-> params, and (3) authenticate the incoming signature.
    #[cfg(feature = "abi")]
    {
        use std::str::FromStr;
        let computed = canonical_submission_id(&record)?;
        let claimed = alloy_primitives::B256::from_str(&record.submission_id)
            .map_err(|_| StoreError::BadField("submission_id"))?;
        if computed != claimed {
            return Err(StoreError::IdMismatch {
                claimed: format!("{claimed:#x}"),
                computed: format!("{computed:#x}"),
            });
        }
        verify_signature(computed, &sig)?;
    }

    let path = file_path(dir, &record.submission_id);

    if path.exists() {
        let existing: SubmissionRecord = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        // (2) Params are immutable once stored: reject any conflicting overwrite,
        // otherwise keep the genuine accumulated signatures.
        if !same_params(&existing, &record) {
            return Err(StoreError::ParamsConflict(record.submission_id.clone()));
        }
        record.signatures = existing.signatures;
    }

    let already = record
        .signatures
        .iter()
        .any(|s| s.signer.eq_ignore_ascii_case(&sig.signer));
    if !already {
        record.signatures.push(sig);
    }

    std::fs::write(&path, serde_json::to_string_pretty(&record)?)?;
    Ok(record)
}

/// Load a single record by submissionId, if present.
pub fn load(dir: &Path, submission_id: &str) -> Result<Option<SubmissionRecord>, StoreError> {
    let path = file_path(dir, submission_id);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&std::fs::read_to_string(&path)?)?))
}

/// Load every record in the store directory.
pub fn load_all(dir: &Path) -> Result<Vec<SubmissionRecord>, StoreError> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(serde_json::from_str(&std::fs::read_to_string(&path)?)?);
        }
    }
    Ok(out)
}

#[cfg(all(test, feature = "abi"))]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::SignerSync;
    use alloy_primitives::{Address, Signature, B256, U256};
    use std::str::FromStr;

    // Encode an alloy signature as 65 bytes r||s||v (v in {27,28}), as the
    // validator does and the Gate expects.
    fn encode_sig(sig: &Signature) -> String {
        let mut out = Vec::with_capacity(65);
        out.extend_from_slice(&sig.r().to_be_bytes::<32>());
        out.extend_from_slice(&sig.s().to_be_bytes::<32>());
        out.push(27 + sig.v() as u8);
        format!("0x{}", hex::encode(out))
    }

    // Build a well-formed record (id == keccak(params)) for a plain transfer.
    fn make_record() -> SubmissionRecord {
        let debridge_id = crate::debridge_id(U256::from(1337u64), Address::repeat_byte(0x11));
        let amount = U256::from(100u64);
        let chain_from = U256::from(1337u64);
        let chain_to = U256::from(1338u64);
        let nonce = U256::from(0u64);
        let receiver = Address::repeat_byte(0xAB).to_vec();
        let id = crate::submission_id(debridge_id, amount, chain_from, chain_to, nonce, &receiver);
        SubmissionRecord {
            submission_id: format!("{id:#x}"),
            debridge_id: format!("{debridge_id:#x}"),
            amount: amount.to_string(),
            chain_id_from: 1337,
            chain_id_to: 1338,
            nonce: 0,
            receiver: format!("0x{}", hex::encode(&receiver)),
            auto_params: "0x".to_string(),
            native_sender: "0x".to_string(),
            signatures: vec![],
        }
    }

    fn sign(signer: &PrivateKeySigner, id_hex: &str) -> SignerSig {
        let id = B256::from_str(id_hex).unwrap();
        let sig = signer.sign_message_sync(id.as_slice()).unwrap();
        SignerSig { signer: format!("{:#x}", signer.address()), signature: encode_sig(&sig) }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!("bridge-store-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn happy_path_two_validators_merge_and_dedupe() {
        let dir = tmp_dir("happy");
        let v1: PrivateKeySigner = PrivateKeySigner::random();
        let v2: PrivateKeySigner = PrivateKeySigner::random();
        let rec = make_record();

        let r = upsert_signature(&dir, rec.clone(), sign(&v1, &rec.submission_id)).unwrap();
        assert_eq!(r.signatures.len(), 1);
        // same validator again -> deduped
        let r = upsert_signature(&dir, rec.clone(), sign(&v1, &rec.submission_id)).unwrap();
        assert_eq!(r.signatures.len(), 1);
        // second validator -> merged
        let r = upsert_signature(&dir, rec.clone(), sign(&v2, &rec.submission_id)).unwrap();
        assert_eq!(r.signatures.len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_id_param_mismatch() {
        // V1 root cause: a record whose submission_id is NOT the hash of its params.
        let dir = tmp_dir("idmismatch");
        let v1 = PrivateKeySigner::random();
        let mut rec = make_record();
        // tamper the amount but keep the (now stale) submission_id
        rec.amount = "999999".to_string();
        let err = upsert_signature(&dir, rec.clone(), sign(&v1, &rec.submission_id)).unwrap_err();
        assert!(matches!(err, StoreError::IdMismatch { .. }), "got {err:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_param_poisoning_of_existing_record() {
        // The headline attack: legit record gets real sigs, then an attacker POSTs
        // the SAME submission_id with different params. Must be rejected, leaving
        // the genuine record intact.
        let dir = tmp_dir("poison");
        let v1 = PrivateKeySigner::random();
        let rec = make_record();
        upsert_signature(&dir, rec.clone(), sign(&v1, &rec.submission_id)).unwrap();

        // attacker reuses the id but lies about the receiver; with a forged sig
        let mut evil = rec.clone();
        evil.receiver = format!("0x{}", hex::encode(Address::repeat_byte(0xEE).to_vec()));
        // keep submission_id pointing at the real file
        let attacker = PrivateKeySigner::random();
        let err = upsert_signature(&dir, evil.clone(), sign(&attacker, &evil.submission_id))
            .unwrap_err();
        // recompute(evil params) != submission_id -> IdMismatch (id<->param binding)
        assert!(matches!(err, StoreError::IdMismatch { .. }), "got {err:?}");

        // the genuine record on disk is untouched
        let on_disk = load(&dir, &rec.submission_id).unwrap().unwrap();
        assert_eq!(on_disk.receiver, rec.receiver);
        assert_eq!(on_disk.signatures.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_forged_signature() {
        // V2: signer field says V1 but the signature is from someone else.
        let dir = tmp_dir("forged");
        let v1 = PrivateKeySigner::random();
        let other = PrivateKeySigner::random();
        let rec = make_record();

        let mut bad = sign(&other, &rec.submission_id);
        bad.signer = format!("{:#x}", v1.address()); // claim to be V1
        let err = upsert_signature(&dir, rec.clone(), bad).unwrap_err();
        assert!(matches!(err, StoreError::SignerMismatch { .. }), "got {err:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_garbage_signature() {
        let dir = tmp_dir("garbage");
        let v1 = PrivateKeySigner::random();
        let rec = make_record();
        let junk = SignerSig {
            signer: format!("{:#x}", v1.address()),
            signature: format!("0x{}", hex::encode([7u8; 65])),
        };
        let err = upsert_signature(&dir, rec.clone(), junk).unwrap_err();
        assert!(
            matches!(err, StoreError::BadSignature | StoreError::SignerMismatch { .. }),
            "got {err:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
