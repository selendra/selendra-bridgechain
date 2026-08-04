//! Where the GraphQL API reads (and optionally writes) signature records: either
//! a local directory or the HTTP `sig-store` service — the same two backends the
//! validator/keeper already speak. Cheap to share behind an `Arc`.

use std::path::PathBuf;
use std::sync::Arc;

use bridge_core::remote::RemoteStore;
use bridge_core::store::{self, SignerSig, SubmissionRecord};
use tokio::sync::Mutex;

/// A read/write view of the shared signature store.
pub enum Backend {
    /// File-per-id directory. The write lock serializes the read-modify-write in
    /// `upsert_signature` (mirrors how the sig-store service guards its dir).
    File { dir: PathBuf, write_lock: Arc<Mutex<()>> },
    /// The HTTP sig-store; it owns the trust-boundary validation server-side.
    Remote(RemoteStore),
}

impl Backend {
    pub fn file(dir: impl Into<PathBuf>) -> Self {
        Backend::File { dir: dir.into(), write_lock: Arc::new(Mutex::new(())) }
    }

    pub fn remote(url: impl Into<String>) -> Self {
        // L-5: read-only credential. The GraphQL API is the most exposed
        // component, so it must hold nothing that can write.
        Backend::Remote(RemoteStore::for_role(url, "SIG_STORE_READER_TOKEN"))
    }

    pub fn describe(&self) -> String {
        match self {
            Backend::File { dir, .. } => format!("file://{}", dir.display()),
            Backend::Remote(_) => "http(sig-store)".into(),
        }
    }

    pub async fn load_all(&self) -> anyhow::Result<Vec<SubmissionRecord>> {
        match self {
            Backend::File { dir, .. } => Ok(store::load_all(dir)?),
            Backend::Remote(remote) => Ok(remote.load_all().await?),
        }
    }

    pub async fn load(&self, submission_id: &str) -> anyhow::Result<Option<SubmissionRecord>> {
        match self {
            Backend::File { dir, .. } => Ok(store::load(dir, submission_id)?),
            Backend::Remote(remote) => Ok(remote.load(submission_id).await?),
        }
    }

    /// Upsert a record + one signature. The dir backend runs the local trust
    /// boundary (`store::upsert_signature`); the remote backend defers to the
    /// sig-store service, which enforces the same checks.
    pub async fn upsert(
        &self,
        record: SubmissionRecord,
        sig: SignerSig,
    ) -> anyhow::Result<SubmissionRecord> {
        match self {
            Backend::File { dir, write_lock } => {
                let _guard = write_lock.lock().await;
                Ok(store::upsert_signature(dir, record, sig)?)
            }
            Backend::Remote(remote) => Ok(remote.upsert(record, sig).await?),
        }
    }
}
