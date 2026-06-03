//! Where the validator writes signatures: a local directory or the HTTP sig-store.
//!
//! Phases 4–6 used a shared file directory. Phase 7 runs N independent
//! validators that each POST to the sig-store service instead; `url` selects it.

use std::path::PathBuf;

use bridge_core::remote::RemoteStore;
use bridge_core::store::{self, SignerSig, SubmissionRecord};

use crate::config::Store;

pub enum Sink {
    File(PathBuf),
    Remote(RemoteStore),
}

impl Sink {
    pub fn from_config(cfg: &Store) -> anyhow::Result<Self> {
        if let Some(url) = &cfg.url {
            Ok(Sink::Remote(RemoteStore::new(url.clone())))
        } else if let Some(dir) = &cfg.dir {
            let dir = PathBuf::from(dir);
            store::ensure_dir(&dir)?;
            Ok(Sink::File(dir))
        } else {
            anyhow::bail!("[store] needs either `dir` or `url`")
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Sink::File(dir) => format!("file://{}", dir.display()),
            Sink::Remote(_) => "http(sig-store)".into(),
        }
    }

    pub async fn upsert(&self, record: SubmissionRecord, sig: SignerSig) -> anyhow::Result<()> {
        match self {
            Sink::File(dir) => {
                store::upsert_signature(dir, record, sig)?;
            }
            Sink::Remote(remote) => {
                remote.upsert(record, sig).await?;
            }
        }
        Ok(())
    }
}
