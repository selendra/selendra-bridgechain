//! The `Submission` — the off-chain mirror of a `Sent` event, plus the
//! independent recomputation of its `submissionId`.

use alloy_primitives::{B256, U256};

use crate::{submission_id, submission_id_with_auto, AutoParams};

/// All parameters of a single cross-chain transfer, as read from a `Sent` event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub debridge_id: B256,
    pub amount: U256,
    pub chain_id_from: U256,
    pub chain_id_to: U256,
    pub nonce: U256,
    pub receiver: Vec<u8>,
    /// `None` for a plain transfer; `Some` when an execution payload is attached.
    pub auto: Option<AutoParams>,
}

impl Submission {
    /// Recompute the submissionId from these parameters (never trust the emitted one).
    pub fn compute_id(&self) -> B256 {
        match &self.auto {
            None => submission_id(
                self.debridge_id,
                self.amount,
                self.chain_id_from,
                self.chain_id_to,
                self.nonce,
                &self.receiver,
            ),
            Some(auto) => submission_id_with_auto(
                self.debridge_id,
                self.amount,
                self.chain_id_from,
                self.chain_id_to,
                self.nonce,
                &self.receiver,
                auto,
            ),
        }
    }
}
