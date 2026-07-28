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
    /// Last accepted nonce, keyed by **(source chain, target chain)**.
    ///
    /// The on-chain nonce is `nonceTo[chainIdTo]` on each SOURCE gate, so every
    /// source produces its own 0,1,2,… sequence toward a given destination. In a
    /// mesh where two sources bridge to the same destination, both legitimately
    /// emit nonce 0 for that `chainIdTo` — keying by `chain_to` alone would flag
    /// the second as a duplicate and wrongly pause. The sequence is unique per
    /// `(chain_from, chain_to)` (both are bound into the submissionId), so that
    /// is the key. Serialized as `{ "<from>": { "<to>": <nonce> } }`.
    pub nonces: BTreeMap<u64, BTreeMap<u64, u64>>,
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
    MissedNonce { chain_from: u64, chain_to: u64, expected: u64, got: u64 },
    DuplicatedNonce { chain_from: u64, chain_to: u64, last: u64, got: u64 },
    IdMismatch { submission_id: String },
}

impl PauseReason {
    pub fn as_str(&self) -> String {
        match self {
            PauseReason::Operator => "operator".into(),
            PauseReason::MissedNonce { chain_from, chain_to, expected, got } => {
                format!("MISSED_NONCE chain_from={chain_from} chain_to={chain_to} expected={expected} got={got}")
            }
            PauseReason::DuplicatedNonce { chain_from, chain_to, last, got } => {
                format!("DUPLICATED_NONCE chain_from={chain_from} chain_to={chain_to} last={last} got={got}")
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
        let fresh = || Persist { last_block: start_block.saturating_sub(1), nonces: BTreeMap::new() };
        let persist = if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            // Tolerate an unreadable/old-format state file: re-scanning from
            // start_block is safe (signing is idempotent — the store dedups by
            // signer), whereas hard-failing would brick the validator. This also
            // covers the pre-mesh `nonces: {"<to>": n}` format transparently.
            serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "unreadable state file; starting fresh");
                fresh()
            })
        } else {
            fresh()
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

    /// Last accepted nonce for a `(chain_from, chain_to)` pair, if any.
    pub fn last_nonce(&self, chain_from: u64, chain_to: u64) -> Option<u64> {
        self.persist.nonces.get(&chain_from).and_then(|m| m.get(&chain_to)).copied()
    }

    /// Decide whether `nonce` for the `(chain_from, chain_to)` corridor is the
    /// expected next one. Pure — does not mutate; call [`Runtime::accept_nonce`]
    /// on Accept.
    pub fn check_nonce(&self, chain_from: u64, chain_to: u64, nonce: u64) -> NonceDecision {
        match self.last_nonce(chain_from, chain_to) {
            // First event ever seen for this corridor: accept (we may be resuming
            // mid-stream; the gap-from-genesis is not meaningful here).
            None => NonceDecision::Accept,
            Some(last) if nonce == last + 1 => NonceDecision::Accept,
            Some(last) if nonce <= last => NonceDecision::Duplicated,
            Some(_) => NonceDecision::Missed,
        }
    }

    pub fn accept_nonce(&mut self, chain_from: u64, chain_to: u64, nonce: u64) {
        self.persist.nonces.entry(chain_from).or_default().insert(chain_to, nonce);
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

    // corridor 1337 -> 1338 in these tests unless noted
    const FROM: u64 = 1337;
    const TO: u64 = 1338;

    #[test]
    fn first_event_for_chain_is_accepted() {
        let r = rt();
        assert_eq!(r.check_nonce(FROM, TO, 0), NonceDecision::Accept);
        assert_eq!(r.check_nonce(FROM, TO, 7), NonceDecision::Accept);
    }

    #[test]
    fn sequential_nonces_accept_then_advance() {
        let mut r = rt();
        assert_eq!(r.check_nonce(FROM, TO, 0), NonceDecision::Accept);
        r.accept_nonce(FROM, TO, 0);
        assert_eq!(r.check_nonce(FROM, TO, 1), NonceDecision::Accept);
        r.accept_nonce(FROM, TO, 1);
        assert_eq!(r.check_nonce(FROM, TO, 2), NonceDecision::Accept);
    }

    #[test]
    fn gap_is_missed() {
        let mut r = rt();
        r.accept_nonce(FROM, TO, 0);
        assert_eq!(r.check_nonce(FROM, TO, 2), NonceDecision::Missed);
    }

    #[test]
    fn replay_is_duplicated() {
        let mut r = rt();
        r.accept_nonce(FROM, TO, 5);
        assert_eq!(r.check_nonce(FROM, TO, 5), NonceDecision::Duplicated);
        assert_eq!(r.check_nonce(FROM, TO, 3), NonceDecision::Duplicated);
    }

    #[test]
    fn nonces_are_independent_per_target_chain() {
        let mut r = rt();
        r.accept_nonce(FROM, TO, 4);
        // a different target chain starts fresh
        assert_eq!(r.check_nonce(FROM, 9999, 0), NonceDecision::Accept);
        // and 1338 still enforces sequence
        assert_eq!(r.check_nonce(FROM, TO, 6), NonceDecision::Missed);
    }

    #[test]
    fn nonces_are_independent_per_source_chain() {
        // THE mesh case: two sources bridging to the SAME destination each run
        // their own 0,1,2,… sequence and must not collide.
        let mut r = rt();
        r.accept_nonce(1337, TO, 0);
        // a second source to the same destination legitimately starts at 0
        assert_eq!(r.check_nonce(1339, TO, 0), NonceDecision::Accept);
        r.accept_nonce(1339, TO, 0);
        // each corridor keeps its own sequence
        assert_eq!(r.check_nonce(1337, TO, 1), NonceDecision::Accept);
        assert_eq!(r.check_nonce(1339, TO, 1), NonceDecision::Accept);
        // and a genuine replay within a corridor is still caught
        assert_eq!(r.check_nonce(1337, TO, 0), NonceDecision::Duplicated);
    }

    #[test]
    fn rescan_resets_cursor_and_nonces() {
        let mut r = rt();
        r.persist.last_block = 100;
        r.accept_nonce(FROM, TO, 9);
        r.pause(PauseReason::Operator);
        r.rescan_from(50);
        assert_eq!(r.next_block(), 50);
        assert!(r.persist.nonces.is_empty());
        assert!(!r.paused);
    }
}
