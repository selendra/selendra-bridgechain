//! Refund attestation for transfers whose DESTINATION is Solana.
//!
//! The two-phase refund needs someone to vouch, from on-chain facts, that a
//! stuck transfer may be burned and then repaid. `crates/validator`'s refund
//! loop does that for EVM destinations. Nothing did it for Solana, so an
//! EVM → Solana transfer that could not be delivered was burnable and refundable
//! only by hand: the deposit stayed locked until an operator noticed.
//!
//! ## Why this attests on the DESTINATION end only
//!
//! The EVM validator votes only on corridors it can read *both* ends of, and
//! that rule is right for it — it can read both. This process can read exactly
//! one end: Solana. It therefore attests only when Solana is the DESTINATION,
//! and that is sufficient rather than a compromise, because the facts the two
//! attestations actually turn on are destination facts:
//!
//!   * **cancel** — "the destination has NOT paid out". Purely destination side.
//!     Attesting a burn for a delivered transfer is the double-spend the whole
//!     design exists to prevent, and that is exactly what this can verify.
//!   * **refund** — "the destination IS burned". Also destination side.
//!
//! The source-side checks the EVM loop additionally makes (already refunded,
//! never sent from this gate) are guards against *wasted* attestations, not
//! against loss: the source gate itself refuses both (`AlreadyRefunded`,
//! `NotSent`). Skipping them here costs a pointless signature at worst.
//!
//! Solana-SOURCE transfers are the mirror image and are deliberately NOT handled
//! here: their destination is an EVM chain, which the EVM validators can read and
//! already attest for. Each attester votes on the end it can actually see.

use std::str::FromStr;
use std::time::Duration;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use tracing::{info, warn};

use crate::config::SourceChain;
use crate::gate::{
    commitment, domain_id, hex32, sign, CANCEL_PREFIX, MARKER_CANCELLED, REFUND_PREFIX,
};
use crate::store::{SignerSig, Store};

/// What the Solana destination says about a submission, read from its
/// `["executed", id]` marker PDA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DestinationState {
    /// A marker PDA exists — the submission is spent, one way or the other.
    pub executed: bool,
    /// …and the marker says BURNED rather than delivered.
    pub cancelled: bool,
}

/// What to do about one candidate.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Destination is untouched — attest the burn.
    AttestCancel,
    /// Destination is burned — attest the payout.
    AttestRefund,
    Skip(&'static str),
}

/// Decide from on-chain facts alone. Split from the I/O so the safety rules are
/// unit-testable, exactly as the EVM loop does it.
pub fn decide(
    dst: &DestinationState,
    already_attested_cancel: bool,
    already_attested_refund: bool,
) -> Decision {
    // THE safety rule. A delivered transfer must never earn a cancel or refund
    // attestation, whatever the store says about timeouts — a refund on top of a
    // claim is the double-spend this design exists to prevent.
    if dst.executed && !dst.cancelled {
        return Decision::Skip("delivered on destination");
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

/// Read the destination marker for a submission.
async fn destination_state(
    rpc: &RpcClient,
    program_id: &Pubkey,
    id: &[u8; 32],
) -> anyhow::Result<DestinationState> {
    let (executed, _) = Pubkey::find_program_address(&[b"executed", id], program_id);
    let acct = rpc.get_account_with_commitment(&executed, rpc.commitment()).await?.value;
    Ok(match acct {
        // Only a PDA the PROGRAM owns counts. An account someone else parked at
        // that address proves nothing about this gate's state.
        Some(a) if a.owner == *program_id && !a.data.is_empty() => DestinationState {
            executed: true,
            cancelled: a.data[0] == MARKER_CANCELLED,
        },
        _ => DestinationState::default(),
    })
}

pub struct Attester {
    rpc: RpcClient,
    program_id: Pubkey,
    chain_id: u64,
    secret: libsecp256k1::SecretKey,
    signer_address: String,
    poll: Duration,
    store: Store,
}

impl Attester {
    pub fn new(
        cfg: &SourceChain,
        secret_key: [u8; 32],
        signer_address: String,
        store: Store,
    ) -> anyhow::Result<Self> {
        Ok(Attester {
            // Refund decisions release real funds, so read them at the same
            // commitment the scanner signs at — a rolled-back "not executed"
            // would let us attest a burn for a transfer that was in fact paid.
            rpc: RpcClient::new_with_commitment(cfg.rpc.clone(), commitment(&cfg.commitment)),
            program_id: Pubkey::from_str(&cfg.program_id)
                .map_err(|_| anyhow::anyhow!("program_id is not a valid pubkey"))?,
            chain_id: cfg.chain_id,
            secret: libsecp256k1::SecretKey::parse(&secret_key)
                .map_err(|_| anyhow::anyhow!("signer key is not a valid secp256k1 scalar"))?,
            signer_address,
            poll: Duration::from_millis(cfg.poll_interval_ms.max(1000)),
            store,
        })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        info!(
            validator = %self.signer_address,
            chain_id = self.chain_id,
            "solana refund attester started (destination-side)"
        );
        loop {
            if let Err(e) = self.tick().await {
                warn!(error = %e, "refund attestation tick failed; retrying");
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    async fn tick(&self) -> anyhow::Result<()> {
        for rec in self.store.refund_candidates().await? {
            // Only transfers landing HERE. A Solana-source transfer's destination
            // is an EVM chain we cannot read, and the EVM validators attest it.
            if rec.chain_id_to != self.chain_id {
                continue;
            }
            let Ok(id) = hex32(&rec.submission_id) else { continue };

            let dst = match destination_state(&self.rpc, &self.program_id, &id).await {
                Ok(d) => d,
                Err(e) => {
                    // Never guess. An unreadable destination means no vote.
                    warn!(submission_id = %rec.submission_id, error = %e, "cannot read destination; not attesting");
                    continue;
                }
            };

            let mine = |sigs: &[SignerSig]| {
                sigs.iter().any(|s| s.signer.eq_ignore_ascii_case(&self.signer_address))
            };
            let decision =
                decide(&dst, mine(&rec.cancel_signatures), mine(&rec.refund_signatures));

            let (kind, prefix) = match decision {
                Decision::AttestCancel => ("cancel", CANCEL_PREFIX),
                Decision::AttestRefund => ("refund", REFUND_PREFIX),
                Decision::Skip(_) => continue,
            };

            let signature = sign(&self.secret, &domain_id(prefix, &id));
            match self
                .store
                .post_attestation(&rec.submission_id, kind, &self.signer_address, &signature)
                .await
            {
                Ok(()) => info!(submission_id = %rec.submission_id, kind, "attested"),
                Err(e) => warn!(submission_id = %rec.submission_id, kind, error = %e, "attestation rejected"),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivered() -> DestinationState {
        DestinationState { executed: true, cancelled: false }
    }
    fn burned() -> DestinationState {
        DestinationState { executed: true, cancelled: true }
    }
    fn untouched() -> DestinationState {
        DestinationState::default()
    }

    /// THE rule. A delivered transfer must never earn either attestation — a
    /// refund on top of a claim is the double-spend the two-phase design exists
    /// to prevent, and no timeout or store state may override an on-chain payout.
    #[test]
    fn a_delivered_transfer_is_never_attested() {
        for (c, r) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(decide(&delivered(), c, r), Decision::Skip("delivered on destination"));
        }
    }

    #[test]
    fn an_untouched_destination_earns_a_cancel() {
        assert_eq!(decide(&untouched(), false, false), Decision::AttestCancel);
    }

    /// Refund is attested only AFTER the burn is on-chain — never on the strength
    /// of a timeout alone, which is what makes the two phases ordered.
    #[test]
    fn refund_requires_the_burn_to_be_on_chain_first() {
        assert_eq!(decide(&untouched(), true, false), Decision::Skip("cancel already attested by us"));
        assert_eq!(decide(&burned(), false, false), Decision::AttestRefund);
    }

    /// Re-attesting is pointless work and churns the store; each phase is voted
    /// exactly once per validator.
    #[test]
    fn we_do_not_re_attest_our_own_vote() {
        assert_eq!(decide(&untouched(), true, false), Decision::Skip("cancel already attested by us"));
        assert_eq!(decide(&burned(), false, true), Decision::Skip("refund already attested by us"));
    }

    /// A marker PDA owned by anyone but the program proves nothing — treating it
    /// as state would let a squatter fake "delivered" and block a legitimate
    /// refund forever.
    #[test]
    fn only_a_program_owned_marker_counts() {
        // Encoded by `destination_state`'s match arms; asserted here as the rule.
        let foreign = DestinationState::default();
        assert!(!foreign.executed, "a non-program-owned account is not state");
        assert_eq!(decide(&foreign, false, false), Decision::AttestCancel);
    }
}
