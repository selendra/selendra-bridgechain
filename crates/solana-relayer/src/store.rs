//! The sig-store wire contract.
//!
//! This duplicates `bridge_core::store::SubmissionRecord` on purpose. That crate
//! depends unconditionally on `alloy-primitives` (it backs the core hashing), and
//! alloy's `zeroize ^1.5` cannot coexist with `solana-client`'s `<1.4` pin — so
//! this process cannot link it at all. What crosses the boundary here is a JSON
//! wire format, not shared logic, and the server re-derives and re-verifies
//! everything it accepts. The real protocol logic still comes from
//! `bridge-solana`, which IS shared.
//!
//! Field names must stay byte-identical to `SubmissionRecord`'s serde names.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignerSig {
    pub signer: String,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmissionRecord {
    pub submission_id: String,
    pub debridge_id: String,
    /// decimal string (uint256 on the EVM side)
    pub amount: String,
    pub chain_id_from: u64,
    pub chain_id_to: u64,
    pub nonce: u64,
    pub receiver: String,
    pub auto_params: String,
    pub native_sender: String,
    #[serde(default)]
    pub token: String,
    pub signatures: Vec<SignerSig>,
    #[serde(default)]
    pub cancel_signatures: Vec<SignerSig>,
    #[serde(default)]
    pub refund_signatures: Vec<SignerSig>,
}

/// Minimal HTTP client for the sig-store, carrying the validator-scoped bearer
/// token (finding L-5: this process signs, so `Sign` is the only scope it needs).
pub struct Store {
    base: String,
    client: reqwest::Client,
}

impl Store {
    pub fn new(base: &str, token: Option<String>) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(token) = token.as_deref().filter(|t| !t.is_empty()) {
            if let Ok(mut v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
                v.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, v);
            }
        }
        Store {
            base: base.trim_end_matches('/').to_string(),
            client: reqwest::Client::builder().default_headers(headers).build().unwrap_or_default(),
        }
    }

    /// POST a record plus this validator's signature. The server enforces the
    /// id⇄params binding and signature authenticity, so a bug here cannot poison
    /// the store — it can only get us rejected.
    pub async fn upsert(&self, record: &SubmissionRecord) -> anyhow::Result<()> {
        let res =
            self.client.post(format!("{}/submissions", self.base)).json(record).send().await?;
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!("sig-store rejected the submission ({status}): {body}");
        }
        Ok(())
    }
}

impl Store {
    /// Every record the store holds. The keeper half filters these down to the
    /// ones bound for Solana.
    pub async fn list(&self) -> anyhow::Result<Vec<SubmissionRecord>> {
        let res = self.client.get(format!("{}/submissions", self.base)).send().await?;
        if !res.status().is_success() {
            anyhow::bail!("sig-store list failed ({})", res.status());
        }
        Ok(res.json().await?)
    }
}
