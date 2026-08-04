//! Where the validator writes signatures: a local directory or the HTTP sig-store.
//!
//! Phases 4–6 used a shared file directory. Phase 7 runs N independent
//! validators that each POST to the sig-store service instead; `url` selects it.

use std::path::PathBuf;

use bridge_core::allow::Allowlist;
use bridge_core::remote::RemoteStore;
use bridge_core::store::{self, SigKind, SignerSig, SubmissionRecord};

use crate::config::Store;

pub enum Sink {
    File(PathBuf),
    Remote(RemoteStore),
}

impl Sink {
    pub fn from_config(cfg: &Store) -> anyhow::Result<Self> {
        if let Some(url) = &cfg.url {
            // L-5: read + sign. Cannot mark claimed or edit the allowlist.
            Ok(Sink::Remote(RemoteStore::for_role(url.clone(), "SIG_STORE_VALIDATOR_TOKEN")))
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

    /// Post a cancel/refund attestation for an already-stored submission.
    pub async fn upsert_attestation(
        &self,
        submission_id: &str,
        kind: SigKind,
        sig: SignerSig,
    ) -> anyhow::Result<()> {
        match self {
            Sink::File(dir) => {
                store::upsert_attestation(dir, submission_id, kind, sig)?;
            }
            Sink::Remote(remote) => {
                remote.upsert_attestation(submission_id, kind, sig).await?;
            }
        }
        Ok(())
    }

    /// Submissions the refund loop should examine. In file mode there is no
    /// server-side lifecycle, so every stored record is offered and the loop's
    /// own on-chain checks do all the filtering.
    pub async fn refund_candidates(&self) -> anyhow::Result<Vec<SubmissionRecord>> {
        match self {
            Sink::File(dir) => Ok(store::load_all(dir)?),
            Sink::Remote(remote) => Ok(remote.refund_candidates().await?),
        }
    }

    /// Fetch the current allowlists from the sig-store, or `None` in legacy file
    /// mode (no central allowlist → enforcement disabled). Built fresh each scan
    /// tick so operator changes take effect without restarting the validator.
    pub async fn fetch_allowlist(&self) -> anyhow::Result<Option<Allowlist>> {
        match self {
            Sink::File(_) => Ok(None),
            Sink::Remote(remote) => {
                let tokens = remote.allowed_tokens().await?;
                let chains = remote.allowed_chains().await?;
                Ok(Some(Allowlist::from_parts(&tokens, &chains)))
            }
        }
    }
}
