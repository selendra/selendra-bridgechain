//! HTTP client for the `sig-store` service (Phase 7).
//!
//! Same record shape as the file-backed [`crate::store`], just over the wire.
//! The validator POSTs its signature; the keeper GETs all records. The server
//! dedupes by signer, so multiple validators converge on one record per id.

use crate::allow::{AllowedChain, AllowedToken, AttestationRequest, ClaimedRequest};
use crate::store::{SigKind, SignerSig, SubmissionRecord};

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
}

pub struct RemoteStore {
    base: String,
    client: reqwest::Client,
}

impl RemoteStore {
    pub fn new(base: impl Into<String>) -> Self {
        // The sig-store is an authenticated trust boundary. If `SIG_STORE_TOKEN`
        // is set, every request carries `Authorization: Bearer <token>` so the
        // server accepts us; unset means the server is in open dev mode.
        Self::with_token(base, std::env::var("SIG_STORE_TOKEN").ok())
    }

    /// Build a client that authenticates with `token` (if `Some`) on every request.
    pub fn with_token(base: impl Into<String>, token: Option<String>) -> Self {
        let base = base.into().trim_end_matches('/').to_string();
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(token) = token.as_deref().filter(|t| !t.is_empty()) {
            if let Ok(mut value) =
                reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            {
                value.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, value);
            }
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap_or_default();
        Self { base, client }
    }

    /// Upsert one signature for a submission (server merges + dedupes by signer).
    pub async fn upsert(
        &self,
        mut record: SubmissionRecord,
        sig: SignerSig,
    ) -> Result<SubmissionRecord, RemoteError> {
        record.signatures = vec![sig];
        let url = format!("{}/submissions", self.base);
        let merged = self
            .client
            .post(url)
            .json(&record)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(merged)
    }

    /// All known records (the keeper polls this).
    pub async fn load_all(&self) -> Result<Vec<SubmissionRecord>, RemoteError> {
        let url = format!("{}/submissions", self.base);
        let all = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(all)
    }

    /// A single record by submissionId, if present.
    pub async fn load(&self, submission_id: &str) -> Result<Option<SubmissionRecord>, RemoteError> {
        let id = submission_id.strip_prefix("0x").unwrap_or(submission_id);
        let url = format!("{}/submissions/{id}", self.base);
        let resp = self.client.get(url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(resp.error_for_status()?.json().await?))
    }

    /// The whitelisted tokens (validator/keeper enforce these before signing/claiming).
    pub async fn allowed_tokens(&self) -> Result<Vec<AllowedToken>, RemoteError> {
        let url = format!("{}/allowed/tokens", self.base);
        Ok(self.client.get(url).send().await?.error_for_status()?.json().await?)
    }

    /// The whitelisted source→target chain pairs.
    pub async fn allowed_chains(&self) -> Result<Vec<AllowedChain>, RemoteError> {
        let url = format!("{}/allowed/chains", self.base);
        Ok(self.client.get(url).send().await?.error_for_status()?.json().await?)
    }

    /// Mark a submission claimed after the keeper executes `claim()` on-chain.
    pub async fn mark_claimed(&self, submission_id: &str, claim_tx: &str) -> Result<(), RemoteError> {
        let id = submission_id.strip_prefix("0x").unwrap_or(submission_id);
        let url = format!("{}/submissions/{id}/claimed", self.base);
        self.client
            .post(url)
            .json(&ClaimedRequest { claim_tx: claim_tx.to_string() })
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    // --- refund path ------------------------------------------------------

    /// Post this validator's cancel/refund attestation. The server re-verifies it
    /// against `kind`'s own digest before counting it.
    pub async fn upsert_attestation(
        &self,
        submission_id: &str,
        kind: SigKind,
        sig: SignerSig,
    ) -> Result<SubmissionRecord, RemoteError> {
        let id = submission_id.strip_prefix("0x").unwrap_or(submission_id);
        let url = format!("{}/submissions/{id}/attestations", self.base);
        let body = AttestationRequest {
            kind: kind.as_str().to_string(),
            signer: sig.signer,
            signature: sig.signature,
        };
        Ok(self.client.post(url).json(&body).send().await?.error_for_status()?.json().await?)
    }

    /// Submissions the refund relayer should examine. Candidates only — the
    /// caller still verifies both chains on-chain before signing anything.
    pub async fn refund_candidates(&self) -> Result<Vec<SubmissionRecord>, RemoteError> {
        let url = format!("{}/refund-candidates", self.base);
        Ok(self.client.get(url).send().await?.error_for_status()?.json().await?)
    }

    // The `cancelled`/`refunded` lifecycle is written only by the indexer from
    // observed on-chain events, so the keeper/relayer client intentionally has no
    // method to set it (that would be reporting a candidate-gating state on the
    // caller's word). See sig-store's router note.
}
