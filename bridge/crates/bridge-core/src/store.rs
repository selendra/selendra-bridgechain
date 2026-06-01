//! A dead-simple, file-backed signature store: one JSON file per submissionId.
//!
//! This is the local-dev stand-in for the deBridge API / Arweave. The validator
//! upserts its signature into `<dir>/<submissionId>.json`; the keeper reads the
//! directory, and once a record has >= threshold signatures, submits `claim()`.
//! Phase 7 replaces this with the HTTP `sig-store` service (same record shape).

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

/// Insert or update a record, merging in `sig` (deduped by signer, case-insensitive).
/// Returns the resulting record (with all known signatures).
pub fn upsert_signature(
    dir: &Path,
    mut record: SubmissionRecord,
    sig: SignerSig,
) -> Result<SubmissionRecord, StoreError> {
    ensure_dir(dir)?;
    let path = file_path(dir, &record.submission_id);

    if path.exists() {
        let existing: SubmissionRecord = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        // keep existing signatures; the params should be identical
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
