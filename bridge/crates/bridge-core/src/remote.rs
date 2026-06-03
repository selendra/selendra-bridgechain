//! HTTP client for the `sig-store` service (Phase 7).
//!
//! Same record shape as the file-backed [`crate::store`], just over the wire.
//! The validator POSTs its signature; the keeper GETs all records. The server
//! dedupes by signer, so multiple validators converge on one record per id.

use crate::store::{SignerSig, SubmissionRecord};

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
        let base = base.into().trim_end_matches('/').to_string();
        Self { base, client: reqwest::Client::new() }
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
}
