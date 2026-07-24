//! Refund attestation loop — the off-chain half of the two-phase refund.
//!
//! A transfer can strand: the destination gate may have no liquidity for the
//! asset, the corridor may be de-listed after the funds were locked, or the
//! target chain may be down long enough that nobody ever claims. The locked
//! funds have to be recoverable, but a refund that merely waits out a timeout is
//! a double-spend — the transfer's claim signatures still exist, so a keeper can
//! deliver on the destination in the same window the source pays the refund.
//!
//! So the refund is ordered, and this loop is what enforces the ordering:
//!
//!   1. The transfer is unclaimed past the timeout, and the DESTINATION gate
//!      still reports `executed == false`. Attest a **cancel**.
//!   2. A keeper submits `cancel()` there, permanently burning the transfer —
//!      `claim()` can never succeed for it again.
//!   3. Only once the destination reports `cancelled == true` do we attest a
//!      **refund**.
//!
//! Step 3 is the load-bearing one. The source gate cannot read the destination,
//! so nothing on-chain stops a refund quorum from paying out early — the
//! guarantee is that this quorum never forms until the burn is an observed
//! on-chain fact. Which is the same trust assumption the bridge already makes
//! for `Sent`: validators attest to what they independently read on a chain.
//!
//! Every decision below is made from on-chain reads at a confirmed block. The
//! store's `refund_status`/timeout only *nominates* candidates; it never
//! authorises anything, so a wrong or manipulated timestamp there can at most
//! cause a wasted look.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use alloy::eips::BlockNumberOrTag;
use alloy::primitives::{Address, B256};
use alloy::providers::{DynProvider, Provider};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use anyhow::Context;
use bridge_core::abi::Gate;
use bridge_core::store::{SigKind, SignerSig, SubmissionRecord};
use tracing::{info, warn};

use crate::config::RefundConfig;
use crate::provider;
use crate::sink::Sink;

/// One chain this validator can independently read gate state from.
struct GateReader {
    chain_id: u64,
    gate: Address,
    provider: DynProvider,
    block_confirmation: u64,
}

impl GateReader {
    async fn connect(
        chain_id: u64,
        gate: &str,
        endpoints: &[String],
        block_confirmation: u64,
    ) -> anyhow::Result<Self> {
        let provider = provider::connect_checked(endpoints, chain_id).await?;
        Ok(GateReader {
            chain_id,
            gate: gate.parse().context("bad gate address")?,
            provider,
            block_confirmation,
        })
    }

    /// The newest block we are willing to trust. Reading `executed` at the chain
    /// tip would let a reorg make a claimed transfer look unclaimed — and this
    /// loop would then attest a cancel for a transfer that was actually paid.
    async fn confirmed_block(&self) -> anyhow::Result<u64> {
        let latest = self.provider.get_block_number().await?;
        Ok(latest.saturating_sub(self.block_confirmation))
    }

    /// Destination-side view of a submission at a confirmed block.
    async fn destination_state(&self, id: B256) -> anyhow::Result<DestinationState> {
        let block = self.confirmed_block().await?;
        let at = BlockNumberOrTag::Number(block).into();
        let gate = Gate::new(self.gate, &self.provider);
        Ok(DestinationState {
            executed: gate.executed(id).block(at).call().await?,
            cancelled: gate.cancelled(id).block(at).call().await?,
        })
    }

    /// Source-side view: who locked the funds (zero once refunded or if this gate
    /// never emitted the id at all), and whether it has already been paid back.
    async fn source_state(&self, id: B256) -> anyhow::Result<SourceState> {
        let block = self.confirmed_block().await?;
        let at = BlockNumberOrTag::Number(block).into();
        let gate = Gate::new(self.gate, &self.provider);
        Ok(SourceState {
            sent_by: gate.sentBy(id).block(at).call().await?,
            refunded: gate.refunded(id).block(at).call().await?,
        })
    }
}

struct DestinationState {
    executed: bool,
    cancelled: bool,
}

struct SourceState {
    sent_by: Address,
    refunded: bool,
}

/// What this validator should do about one candidate, after reading both chains.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Destination is unclaimed and past the timeout — attest the burn.
    AttestCancel,
    /// Destination is burned — attest the payout.
    AttestRefund,
    /// Nothing to do (delivered, already refunded, already attested by us, or a
    /// chain we don't watch). Carries a reason for the log.
    Skip(&'static str),
}

/// Decide from on-chain facts alone. Split out from the I/O so the safety rules
/// are unit-testable.
fn decide(
    src: &SourceState,
    dst: &DestinationState,
    already_attested_cancel: bool,
    already_attested_refund: bool,
) -> Decision {
    // The destination was DELIVERED. Never attest anything: a refund on top of a
    // claim is the double-spend this whole design exists to prevent.
    if dst.executed && !dst.cancelled {
        return Decision::Skip("delivered on destination");
    }
    // Source has already paid it back.
    if src.refunded || src.sent_by == Address::ZERO {
        return Decision::Skip("already refunded, or not sent from this gate");
    }
    if dst.cancelled {
        return if already_attested_refund {
            Decision::Skip("refund already attested by us")
        } else {
            Decision::AttestRefund
        };
    }
    if already_attested_cancel {
        return Decision::Skip("cancel already attested by us");
    }
    Decision::AttestCancel
}

/// Poll the store for stuck transfers and attest cancels/refunds for them.
pub async fn run(
    cfg: RefundConfig,
    sources: Vec<(u64, String, Vec<String>)>, // (chain_id, gate, endpoints)
    signer: PrivateKeySigner,
    sink: std::sync::Arc<Sink>,
) -> anyhow::Result<()> {
    let signer_addr = signer.address();
    let retry = Duration::from_millis(cfg.poll_interval_ms.max(1000));

    // Connect every chain up front; a validator that cannot read a chain must not
    // vote on transfers touching it, so a failure here is fatal to the loop
    // rather than something to paper over.
    let mut source_readers: BTreeMap<u64, GateReader> = BTreeMap::new();
    for (chain_id, gate, endpoints) in &sources {
        let reader = GateReader::connect(*chain_id, gate, endpoints, cfg.block_confirmation).await?;
        source_readers.insert(*chain_id, reader);
    }

    let mut dest_readers: BTreeMap<u64, GateReader> = BTreeMap::new();
    for dest in &cfg.destinations {
        let endpoints = dest.endpoints()?;
        let reader =
            GateReader::connect(dest.chain_id, &dest.gate, &endpoints, cfg.block_confirmation).await?;
        dest_readers.insert(dest.chain_id, reader);
    }

    info!(
        validator = %signer_addr,
        sources = source_readers.len(),
        destinations = dest_readers.len(),
        timeout_secs = cfg.timeout_secs,
        "refund attestation loop started"
    );

    loop {
        let candidates = match sink.refund_candidates().await {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "fetching refund candidates failed; retrying");
                tokio::time::sleep(retry).await;
                continue;
            }
        };

        for rec in candidates {
            if let Err(e) =
                handle_candidate(&rec, &source_readers, &dest_readers, &signer, signer_addr, &sink).await
            {
                warn!(submission_id = %rec.submission_id, error = %e, "refund attestation failed");
            }
        }

        tokio::time::sleep(Duration::from_millis(cfg.poll_interval_ms)).await;
    }
}

async fn handle_candidate(
    rec: &SubmissionRecord,
    source_readers: &BTreeMap<u64, GateReader>,
    dest_readers: &BTreeMap<u64, GateReader>,
    signer: &PrivateKeySigner,
    signer_addr: Address,
    sink: &Sink,
) -> anyhow::Result<()> {
    // Only vote on corridors we can verify BOTH ends of. Attesting on a chain we
    // cannot read would mean trusting the store's word for whether a transfer was
    // delivered — precisely the thing an attacker would want us to do.
    let Some(src) = source_readers.get(&rec.chain_id_from) else { return Ok(()) };
    let Some(dst) = dest_readers.get(&rec.chain_id_to) else { return Ok(()) };

    let id = B256::from_str(&rec.submission_id).context("bad submission_id")?;

    let src_state = src.source_state(id).await.context("reading source gate")?;
    let dst_state = dst.destination_state(id).await.context("reading destination gate")?;

    let mine = |sigs: &[SignerSig]| sigs.iter().any(|s| s.signer.eq_ignore_ascii_case(&format!("{signer_addr:#x}")));
    let decision = decide(
        &src_state,
        &dst_state,
        mine(&rec.cancel_signatures),
        mine(&rec.refund_signatures),
    );

    let kind = match decision {
        Decision::Skip(reason) => {
            tracing::debug!(submission_id = %rec.submission_id, reason, "no attestation");
            return Ok(());
        }
        Decision::AttestCancel => SigKind::Cancel,
        Decision::AttestRefund => SigKind::Refund,
    };

    let digest = kind.digest(id);
    let sig = signer.sign_message(digest.as_slice()).await?;
    let sig = SignerSig {
        signer: format!("{signer_addr:#x}"),
        signature: encode_signature(&sig),
    };

    sink.upsert_attestation(&rec.submission_id, kind, sig).await?;

    info!(
        submission_id = %rec.submission_id,
        kind = kind.as_str(),
        chain_from = rec.chain_id_from,
        chain_to = rec.chain_id_to,
        source_chain = src.chain_id,
        dest_chain = dst.chain_id,
        "ATTESTED"
    );
    Ok(())
}

/// 65 bytes r||s||v with v in {27,28} — the same encoding the transfer path uses.
fn encode_signature(sig: &alloy::primitives::Signature) -> String {
    let mut out = Vec::with_capacity(65);
    out.extend_from_slice(&sig.r().to_be_bytes::<32>());
    out.extend_from_slice(&sig.s().to_be_bytes::<32>());
    out.push(27 + sig.v() as u8);
    format!("0x{}", hex::encode(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender() -> Address {
        Address::repeat_byte(0x11)
    }

    fn src(refunded: bool) -> SourceState {
        SourceState { sent_by: sender(), refunded }
    }

    #[test]
    fn never_attests_a_delivered_transfer() {
        // THE safety rule. A claimed transfer must never earn a cancel or refund
        // attestation, whatever the store says about timeouts.
        let dst = DestinationState { executed: true, cancelled: false };
        assert!(matches!(decide(&src(false), &dst, false, false), Decision::Skip(_)));
    }

    #[test]
    fn attests_cancel_when_destination_is_untouched() {
        let dst = DestinationState { executed: false, cancelled: false };
        assert_eq!(decide(&src(false), &dst, false, false), Decision::AttestCancel);
    }

    #[test]
    fn attests_refund_only_after_the_burn_is_on_chain() {
        let untouched = DestinationState { executed: false, cancelled: false };
        assert_eq!(decide(&src(false), &untouched, true, false), Decision::Skip("cancel already attested by us"));

        let burned = DestinationState { executed: true, cancelled: true };
        assert_eq!(decide(&src(false), &burned, true, false), Decision::AttestRefund);
    }

    #[test]
    fn stops_once_the_source_has_paid_out() {
        let burned = DestinationState { executed: true, cancelled: true };
        assert!(matches!(decide(&src(true), &burned, true, true), Decision::Skip(_)));
    }

    #[test]
    fn refuses_a_submission_this_gate_never_sent() {
        // A quorum must not form for a transfer that was never locked here.
        let burned = DestinationState { executed: true, cancelled: true };
        let ghost = SourceState { sent_by: Address::ZERO, refunded: false };
        assert!(matches!(decide(&ghost, &burned, false, false), Decision::Skip(_)));
    }

    #[test]
    fn does_not_re_attest() {
        let burned = DestinationState { executed: true, cancelled: true };
        assert_eq!(
            decide(&src(false), &burned, true, true),
            Decision::Skip("refund already attested by us")
        );
    }
}
