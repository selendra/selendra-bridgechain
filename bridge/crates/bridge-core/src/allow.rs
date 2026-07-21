//! Allowlists — which tokens and which source→target chain pairs may bridge.
//!
//! These types are the on-wire shape shared by the Postgres-backed `sig-store`
//! (server) and the `RemoteStore` HTTP client the validator/keeper use to fetch
//! the lists. The DB (`bridge-db`) is the source of truth; everyone else mirrors
//! it into an in-memory [`Allowlist`] for fast membership checks.
//!
//! ## Semantics: the allowlist is OPT-IN
//!
//! An *empty* token list means "no token restriction configured" — every token
//! is allowed. The first row you add flips it to deny-by-default: only listed
//! tokens pass. The chain list behaves the same way, independently. This keeps
//! every existing end-to-end script working until an operator deliberately seeds
//! the lists, then enforcement turns on with no code change.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// One whitelisted ERC-20, keyed by `(chain_id, token)`. `debridge_id` is the
/// `keccak256(chainId, token)` the Gate emits in `Sent` — precomputed so the
/// validator/keeper can match an event by a single hash lookup.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedToken {
    pub chain_id: u64,
    /// `0x`-prefixed token address (lowercased).
    pub token: String,
    /// `0x`-prefixed `keccak256(abi.encodePacked(chain_id, token))` (lowercased).
    pub debridge_id: String,
    #[serde(default)]
    pub symbol: Option<String>,
}

/// One whitelisted directed chain pair: transfers from `chain_id_from` to
/// `chain_id_to` are permitted.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedChain {
    pub chain_id_from: u64,
    pub chain_id_to: u64,
}

/// Request body to add a token to the allowlist; the `debridge_id` is derived
/// server-side so a caller can't pin a token onto the wrong hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddTokenRequest {
    pub chain_id: u64,
    pub token: String,
    #[serde(default)]
    pub symbol: Option<String>,
}

/// Request body to mark a submission claimed (keeper → sig-store).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimedRequest {
    /// `0x`-prefixed claim transaction hash on the target chain.
    pub claim_tx: String,
}

/// A submission as it appears in the transaction-history view: the transfer
/// parameters plus its lifecycle status and timing. Distinct from
/// [`crate::store::SubmissionRecord`] (which carries the raw signatures the
/// keeper needs) so history queries stay cheap and read-only.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmissionHistory {
    pub submission_id: String,
    pub debridge_id: String,
    pub amount: String,
    pub chain_id_from: u64,
    pub chain_id_to: u64,
    pub nonce: u64,
    pub receiver: String,
    /// `signed` once at least one validator has attested; `claimed` after the
    /// keeper executes `claim()` on the target chain.
    pub status: String,
    /// Target-chain claim tx hash, once claimed.
    pub claim_tx: Option<String>,
    pub signature_count: i64,
    /// RFC-3339 timestamps.
    pub created_at: String,
    pub updated_at: String,
}

/// In-memory mirror of the allowlists, built from the fetched rows for O(1)
/// membership checks on the hot path (validator signing / keeper claiming).
#[derive(Clone, Debug, Default)]
pub struct Allowlist {
    /// Allowed `debridge_id`s (lowercased `0x`-hex).
    debridge_ids: HashSet<String>,
    /// Allowed `(chain_id_from, chain_id_to)` pairs.
    chains: HashSet<(u64, u64)>,
}

impl Allowlist {
    /// Build from the lists fetched from the store.
    pub fn from_parts(tokens: &[AllowedToken], chains: &[AllowedChain]) -> Self {
        Allowlist {
            debridge_ids: tokens.iter().map(|t| t.debridge_id.to_ascii_lowercase()).collect(),
            chains: chains.iter().map(|c| (c.chain_id_from, c.chain_id_to)).collect(),
        }
    }

    /// Opt-in semantics: an empty token list allows everything; otherwise only
    /// listed `debridge_id`s pass. `debridge_id` is matched case-insensitively.
    pub fn token_allowed(&self, debridge_id: &str) -> bool {
        self.debridge_ids.is_empty() || self.debridge_ids.contains(&debridge_id.to_ascii_lowercase())
    }

    /// Opt-in semantics: an empty chain list allows every pair; otherwise only
    /// listed `(from, to)` pairs pass.
    pub fn chain_allowed(&self, from: u64, to: u64) -> bool {
        self.chains.is_empty() || self.chains.contains(&(from, to))
    }
}
