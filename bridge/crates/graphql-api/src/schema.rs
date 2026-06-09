//! The GraphQL schema: a read view over the signature store plus a single
//! `submitSignature` mutation that goes through the same trust-boundary `upsert`.
//!
//! Nothing here talks to a chain — it reports what the validators have *signed*,
//! not what the keeper has executed on-chain. `meetsThreshold` therefore means
//! "has enough signatures for the keeper to claim", given the `--threshold` the
//! API was started with (omitted => `meetsThreshold` is null).

use std::sync::Arc;

use async_graphql::{ComplexObject, Context, Enum, InputObject, Object, SimpleObject};
use bridge_core::store::{SignerSig, SubmissionRecord};

use crate::backend::Backend;
use crate::chain::Chains;

/// Shared, read-mostly state handed to every resolver via the schema's data.
pub struct ApiState {
    pub backend: Arc<Backend>,
    /// Signature count the keeper requires to claim. `None` => unknown here, so
    /// `meetsThreshold`/`ready` are reported as null/zero.
    pub threshold: Option<u64>,
    /// Optional destination-gate RPCs, so `executed`/`status` can report on-chain
    /// delivery. Empty => those fields are null/UNKNOWN.
    pub chains: Chains,
}

/// Lifecycle of a transfer, combining off-chain signatures with on-chain truth.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SubmissionStatus {
    /// Fewer than `threshold` signatures collected — the keeper can't claim yet.
    Pending,
    /// Enough signatures to claim, but not yet executed on the destination chain.
    Ready,
    /// `executed(submissionId) == true` on the destination gate — delivered.
    Executed,
    /// Can't be determined (no `--threshold` and/or no destination RPC configured).
    Unknown,
}

/// One validator's signature over a submissionId.
#[derive(SimpleObject)]
pub struct Signature {
    /// Recovered signer address, `0x`-prefixed.
    pub signer: String,
    /// 65-byte ECDSA signature (r||s||v), `0x`-prefixed.
    pub signature: String,
}

impl From<SignerSig> for Signature {
    fn from(s: SignerSig) -> Self {
        Signature { signer: s.signer, signature: s.signature }
    }
}

/// A cross-chain transfer and the signatures collected for it so far.
///
/// The plain fields come straight from the store (cheap). `executed` and `status`
/// are resolved lazily — they only hit the destination chain when a client asks
/// for them, and only if the API was started with a `--gate` for `chainIdTo`.
#[derive(SimpleObject)]
#[graphql(complex)]
pub struct Submission {
    /// The sacred submissionId (`0x`-prefixed keccak of the transfer params).
    pub submission_id: String,
    pub debridge_id: String,
    /// uint256 as a decimal string (avoids JSON precision loss).
    pub amount: String,
    pub chain_id_from: u64,
    pub chain_id_to: u64,
    pub nonce: u64,
    /// `0x`-prefixed raw receiver bytes.
    pub receiver: String,
    /// `0x`-prefixed execution payload (`0x` when none).
    pub auto_params: String,
    /// `0x`-prefixed packed source sender.
    pub native_sender: String,
    pub signatures: Vec<Signature>,
    /// Convenience: `signatures.len()`.
    pub signature_count: u64,
    /// `signatureCount >= threshold`, or null if the API has no threshold set.
    pub meets_threshold: Option<bool>,
}

impl Submission {
    fn from_record(rec: SubmissionRecord, threshold: Option<u64>) -> Self {
        let signature_count = rec.signatures.len() as u64;
        let meets_threshold = threshold.map(|t| signature_count >= t);
        Submission {
            submission_id: rec.submission_id,
            debridge_id: rec.debridge_id,
            amount: rec.amount,
            chain_id_from: rec.chain_id_from,
            chain_id_to: rec.chain_id_to,
            nonce: rec.nonce,
            receiver: rec.receiver,
            auto_params: rec.auto_params,
            native_sender: rec.native_sender,
            signatures: rec.signatures.into_iter().map(Into::into).collect(),
            signature_count,
            meets_threshold,
        }
    }
}

#[ComplexObject]
impl Submission {
    /// On-chain `executed(submissionId)` on the destination gate. `null` when the
    /// API has no `--gate` configured for `chainIdTo` (or the RPC call failed).
    async fn executed(&self, ctx: &Context<'_>) -> Option<bool> {
        state(ctx).chains.executed(self.chain_id_to, &self.submission_id).await
    }

    /// Combined lifecycle: EXECUTED if the destination gate confirms it,
    /// otherwise READY/PENDING from the signature count, or UNKNOWN if neither
    /// a threshold nor a destination RPC is configured.
    async fn status(&self, ctx: &Context<'_>) -> SubmissionStatus {
        if state(ctx).chains.executed(self.chain_id_to, &self.submission_id).await == Some(true) {
            return SubmissionStatus::Executed;
        }
        match self.meets_threshold {
            Some(true) => SubmissionStatus::Ready,
            Some(false) => SubmissionStatus::Pending,
            None => SubmissionStatus::Unknown,
        }
    }
}

/// Optional filters for `submissions`. All supplied fields must match (AND).
#[derive(InputObject, Default)]
pub struct SubmissionFilter {
    pub chain_id_from: Option<u64>,
    pub chain_id_to: Option<u64>,
    /// Keep only records with at least this many signatures.
    pub min_signatures: Option<u64>,
    /// `true` => only records that meet the keeper threshold; `false` => only
    /// those that don't. Requires the API to have been started with `--threshold`.
    pub ready: Option<bool>,
}

/// How many submissions flow along one source→destination route.
#[derive(SimpleObject)]
pub struct RouteCount {
    pub chain_id_from: u64,
    pub chain_id_to: u64,
    pub count: u64,
}

/// Aggregate view of the whole store.
#[derive(SimpleObject)]
pub struct Stats {
    /// Total records in the store.
    pub total: u64,
    /// Records with at least one signature.
    pub signed: u64,
    /// Records that meet the threshold (0 if no threshold is configured).
    pub ready: u64,
    /// The configured keeper threshold, if any.
    pub threshold: Option<u64>,
    /// Per source→destination route counts, sorted by (from, to).
    pub routes: Vec<RouteCount>,
}

fn state<'c>(ctx: &Context<'c>) -> &'c ApiState {
    ctx.data_unchecked::<ApiState>()
}

pub struct Query;

#[Object]
impl Query {
    /// All submissions, newest filters applied. Sorted by (chainIdFrom,
    /// chainIdTo, nonce) for a stable order.
    async fn submissions(
        &self,
        ctx: &Context<'_>,
        filter: Option<SubmissionFilter>,
    ) -> async_graphql::Result<Vec<Submission>> {
        let st = state(ctx);
        let f = filter.unwrap_or_default();
        let threshold = st.threshold;

        let mut records = st.backend.load_all().await?;
        records.sort_by(|a, b| {
            (a.chain_id_from, a.chain_id_to, a.nonce).cmp(&(
                b.chain_id_from,
                b.chain_id_to,
                b.nonce,
            ))
        });

        let out = records
            .into_iter()
            .filter(|r| f.chain_id_from.is_none_or(|c| r.chain_id_from == c))
            .filter(|r| f.chain_id_to.is_none_or(|c| r.chain_id_to == c))
            .filter(|r| f.min_signatures.is_none_or(|m| r.signatures.len() as u64 >= m))
            .filter(|r| match (f.ready, threshold) {
                (Some(want), Some(t)) => (r.signatures.len() as u64 >= t) == want,
                (Some(_), None) => false, // asked to filter by readiness but we can't judge
                (None, _) => true,
            })
            .map(|r| Submission::from_record(r, threshold))
            .collect();
        Ok(out)
    }

    /// A single submission by its `0x`-prefixed submissionId, or null if unknown.
    async fn submission(
        &self,
        ctx: &Context<'_>,
        submission_id: String,
    ) -> async_graphql::Result<Option<Submission>> {
        // Reject anything that isn't a 32-byte hex hash before it reaches a
        // backend, where it would otherwise build a file path (dir) or a URL
        // (remote) — path-traversal / URL-injection defense at the boundary.
        if !bridge_core::store::is_valid_submission_id(&submission_id) {
            return Err(async_graphql::Error::new(
                "submissionId must be a 32-byte hex hash (0x + 64 hex digits)",
            ));
        }
        let st = state(ctx);
        let rec = st.backend.load(&submission_id).await?;
        Ok(rec.map(|r| Submission::from_record(r, st.threshold)))
    }

    /// Aggregate counts across the whole store.
    async fn stats(&self, ctx: &Context<'_>) -> async_graphql::Result<Stats> {
        use std::collections::BTreeMap;
        let st = state(ctx);
        let records = st.backend.load_all().await?;

        let mut signed = 0u64;
        let mut ready = 0u64;
        let mut routes: BTreeMap<(u64, u64), u64> = BTreeMap::new();
        for r in &records {
            let n = r.signatures.len() as u64;
            if n >= 1 {
                signed += 1;
            }
            if let Some(t) = st.threshold {
                if n >= t {
                    ready += 1;
                }
            }
            *routes.entry((r.chain_id_from, r.chain_id_to)).or_default() += 1;
        }

        Ok(Stats {
            total: records.len() as u64,
            signed,
            ready,
            threshold: st.threshold,
            routes: routes
                .into_iter()
                .map(|((from, to), count)| RouteCount {
                    chain_id_from: from,
                    chain_id_to: to,
                    count,
                })
                .collect(),
        })
    }
}

/// Mirrors a `SubmissionRecord` plus exactly one signature to merge in.
#[derive(InputObject)]
pub struct SubmissionInput {
    pub submission_id: String,
    pub debridge_id: String,
    pub amount: String,
    pub chain_id_from: u64,
    pub chain_id_to: u64,
    pub nonce: u64,
    pub receiver: String,
    /// `0x` when there is no execution payload.
    pub auto_params: String,
    pub native_sender: String,
    /// The signer address for the attached signature, `0x`-prefixed.
    pub signer: String,
    /// 65-byte ECDSA signature (r||s||v), `0x`-prefixed.
    pub signature: String,
}

pub struct Mutation;

#[Object]
impl Mutation {
    /// Upsert a transfer record and merge in one validator signature. Rejected
    /// (by the same trust boundary the sig-store uses) unless the submissionId
    /// equals the keccak of the params and the signature recovers to `signer`.
    async fn submit_signature(
        &self,
        ctx: &Context<'_>,
        input: SubmissionInput,
    ) -> async_graphql::Result<Submission> {
        let st = state(ctx);
        let sig = SignerSig { signer: input.signer, signature: input.signature };
        let record = SubmissionRecord {
            submission_id: input.submission_id,
            debridge_id: input.debridge_id,
            amount: input.amount,
            chain_id_from: input.chain_id_from,
            chain_id_to: input.chain_id_to,
            nonce: input.nonce,
            receiver: input.receiver,
            auto_params: input.auto_params,
            native_sender: input.native_sender,
            signatures: Vec::new(),
        };
        let merged = st.backend.upsert(record, sig).await?;
        Ok(Submission::from_record(merged, st.threshold))
    }
}
