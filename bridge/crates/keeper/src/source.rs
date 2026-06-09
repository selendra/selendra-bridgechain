//! Where the keeper reads signatures from: a local directory or the HTTP sig-store.

use std::path::PathBuf;

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
}
