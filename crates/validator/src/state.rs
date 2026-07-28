//! Resumable cursor + sequential-nonce enforcement + pause state.
//!
//! Mirrors three pieces of the production node:
//!   * the per-chain DB cursor (`supported_chains.latestBlock`) → here a JSON file,
//!   * `NonceControllingService` (MISSED_NONCE / DUPLICATED_NONCE → pause),
//!   * the scanner pause/resume flag driven by the operator API.
//!
//! The whole struct is shared behind a `tokio::sync::Mutex`; the scan loop and
//! the operator API both mutate it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The part that survives a restart.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Persist {
    /// Last block whose logs were fully processed (resume from `last_block + 1`).
    pub last_block: u64,
    /// Per-target-chain (`chainIdTo`) last accepted nonce.
    pub nonces: BTreeMap<u64, u64>,
}

/// What the nonce checker decides for a freshly seen event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NonceDecision {
    /// Expected next nonce — process it.
    Accept,
    /// `nonce <= last seen` — already processed (possible RPC replay).
    Duplicated,
    /// `nonce > last + 1` — a gap; we missed an event.
    Missed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PauseReason {
    Operator,
    MissedNonce { chain_to: u64, expected: u64, got: u64 },
    DuplicatedNonce { chain_to: u64, last: u64, got: u64 },
    IdMismatch { submission_id: String },
}

impl PauseReason {
    pub fn as_str(&self) -> String {
        match self {
            PauseReason::Operator => "operator".into(),
            PauseReason::MissedNonce { chain_to, expected, got } => {
                format!("MISSED_NONCE chain_to={chain_to} expected={expected} got={got}")
            }
            PauseReason::DuplicatedNonce { chain_to, last, got } => {
                format!("DUPLICATED_NONCE chain_to={chain_to} last={last} got={got}")
            }
            PauseReason::IdMismatch { submission_id } => {
                format!("ID_MISMATCH submission_id={submission_id}")
            }
        }
    }
}

pub struct Runtime {
    pub persist: Persist,
    pub paused: bool,
    pub pause_reason: Option<PauseReason>,
    path: PathBuf,
}

impl Runtime {
    /// Load persisted state if present, else start fresh from `start_block`
    /// (the first block we'd scan is `last_block + 1`, so seed `last_block`
    /// with `start_block.saturating_sub(1)`).
    pub fn load_or_init(path: &Path, start_block: u64) -> anyhow::Result<Self> {
        let persist = if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            serde_json::from_str(&raw)?
        } else {
            Persist { last_block: start_block.saturating_sub(1), nonces: BTreeMap::new() }
        };
        Ok(Self { persist, paused: false, pause_reason: None, path: path.to_path_buf() })
    }

    pub fn next_block(&self) -> u64 {
        self.persist.last_block + 1
    }

    /// Atomically persist (write temp + rename) so a crash mid-write can't
    /// corrupt the cursor.
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.persist)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Decide whether `nonce` (for target chain `chain_to`) is the expected next
    /// one. Pure — does not mutate; call [`Runtime::accept_nonce`] on Accept.
    pub fn check_nonce(&self, chain_to: u64, nonce: u64) -> NonceDecision {
        match self.persist.nonces.get(&chain_to) {
            // First event ever seen for this target chain: accept (we may be
            // resuming mid-stream; the gap-from-genesis is not meaningful here).
            None => NonceDecision::Accept,
            Some(&last) if nonce == last + 1 => NonceDecision::Accept,
            Some(&last) if nonce <= last => NonceDecision::Duplicated,
            Some(_) => NonceDecision::Missed,
        }
    }

    pub fn accept_nonce(&mut self, chain_to: u64, nonce: u64) {
        self.persist.nonces.insert(chain_to, nonce);
    }

    pub fn pause(&mut self, reason: PauseReason) {
        self.paused = true;
        self.pause_reason = Some(reason);
    }

    /// Clear the pause flag (operator resume).
    pub fn resume(&mut self) {
        self.paused = false;
        self.pause_reason = None;
    }

    /// Reset the cursor to re-scan from `from_block` and clear nonce tracking so
    /// re-processing is clean (operator rescan). Also clears any pause.
    pub fn rescan_from(&mut self, from_block: u64) {
        self.persist.last_block = from_block.saturating_sub(1);
        self.persist.nonces.clear();
        self.resume();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt() -> Runtime {
        Runtime {
            persist: Persist::default(),
            paused: false,
            pause_reason: None,
            path: PathBuf::from("/dev/null"),
        }
    }

    #[test]
    fn first_event_for_chain_is_accepted() {
        let r = rt();
        assert_eq!(r.check_nonce(1338, 0), NonceDecision::Accept);
        assert_eq!(r.check_nonce(1338, 7), NonceDecision::Accept);
    }

    #[test]
    fn sequential_nonces_accept_then_advance() {
        let mut r = rt();
        assert_eq!(r.check_nonce(1338, 0), NonceDecision::Accept);
        r.accept_nonce(1338, 0);
        assert_eq!(r.check_nonce(1338, 1), NonceDecision::Accept);
        r.accept_nonce(1338, 1);
        assert_eq!(r.check_nonce(1338, 2), NonceDecision::Accept);
    }

    #[test]
    fn gap_is_missed() {
        let mut r = rt();
        r.accept_nonce(1338, 0);
        assert_eq!(r.check_nonce(1338, 2), NonceDecision::Missed);
    }

    #[test]
    fn replay_is_duplicated() {
        let mut r = rt();
        r.accept_nonce(1338, 5);
        assert_eq!(r.check_nonce(1338, 5), NonceDecision::Duplicated);
        assert_eq!(r.check_nonce(1338, 3), NonceDecision::Duplicated);
    }

    #[test]
    fn nonces_are_independent_per_target_chain() {
        let mut r = rt();
        r.accept_nonce(1338, 4);
        // a different target chain starts fresh
        assert_eq!(r.check_nonce(9999, 0), NonceDecision::Accept);
        // and 1338 still enforces sequence
        assert_eq!(r.check_nonce(1338, 6), NonceDecision::Missed);
    }

    #[test]
    fn rescan_resets_cursor_and_nonces() {
        let mut r = rt();
        r.persist.last_block = 100;
        r.accept_nonce(1338, 9);
        r.pause(PauseReason::Operator);
        r.rescan_from(50);
        assert_eq!(r.next_block(), 50);
        assert!(r.persist.nonces.is_empty());
        assert!(!r.paused);
    }
}
