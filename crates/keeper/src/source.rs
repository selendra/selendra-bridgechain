//! Where the keeper reads signatures from: a local directory or the HTTP sig-store.

use std::path::PathBuf;

use bridge_core::allow::Allowlist;
use bridge_core::remote::RemoteStore;
use bridge_core::store::{self, SubmissionRecord};

use crate::config::Store;

/// A read source for signature records. Cheap to share across the per-target
/// claim loops (wrap in an `Arc`); `RemoteStore` holds a reusable HTTP client.
pub enum Source {
    File(PathBuf),
    Remote(RemoteStore),
}

impl Source {
    pub fn from_config(cfg: &Store) -> anyhow::Result<Self> {
        if let Some(url) = &cfg.url {
            Ok(Source::Remote(RemoteStore::new(url.clone())))
        } else if let Some(dir) = &cfg.dir {
            Ok(Source::File(PathBuf::from(dir)))
        } else {
            anyhow::bail!("[store] needs either `dir` or `url`")
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Source::File(dir) => format!("file://{}", dir.display()),
            Source::Remote(_) => "http(sig-store)".into(),
        }
    }

    pub async fn load_all(&self) -> anyhow::Result<Vec<SubmissionRecord>> {
        match self {
            Source::File(dir) => Ok(store::load_all(dir)?),
            Source::Remote(remote) => Ok(remote.load_all().await?),
        }
    }

    /// Current allowlists from the sig-store, or `None` in legacy file mode
    /// (enforcement disabled). Refetched each tick so operator changes apply live.
    pub async fn fetch_allowlist(&self) -> anyhow::Result<Option<Allowlist>> {
        match self {
            Source::File(_) => Ok(None),
            Source::Remote(remote) => {
                let tokens = remote.allowed_tokens().await?;
                let chains = remote.allowed_chains().await?;
                Ok(Some(Allowlist::from_parts(&tokens, &chains)))
            }
        }
    }

    /// Record a successful claim back to the store (no-op in file mode).
    pub async fn mark_claimed(&self, submission_id: &str, claim_tx: &str) -> anyhow::Result<()> {
        match self {
            Source::File(_) => Ok(()),
            Source::Remote(remote) => Ok(remote.mark_claimed(submission_id, claim_tx).await?),
        }
    }

}
