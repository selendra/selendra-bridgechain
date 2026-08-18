//! The Solana source scanner: the missing runner from finding M-3.
//!
//! Does exactly what the EVM validator's `scan_source` does, against a different
//! VM: poll for the gate's transactions, decode each `Sent` event, INDEPENDENTLY
//! recompute the submissionId, sign it only if it matches, and store the
//! signature. It signs with the same secp256k1 key the validator uses on the EVM
//! side, so one validator set attests for both chains.
//!
//! The two safety rules carried over from the EVM scanner:
//!   * **finality** — read only at `finalized`, so a fork cannot discard a `Sent`
//!     after the destination has paid out (enforced in [`crate::config`]);
//!   * **never sign what you cannot reproduce** — a mismatch between the emitted
//!     and recomputed id means a lying RPC or a divergent program, and is a hard
//!     stop rather than a skip.

use std::str::FromStr;
use std::time::Duration;

use anyhow::Context as _;
use bridge_solana::gate::Sent;
use bridge_solana::hash::{amount_word, submission_id, submission_id_with_auto};
use bridge_solana::relayer::{
    gate_program_data_lines, parse_sent_event_line, verify_sent_record, SentEvent,
};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_transaction_status::UiTransactionEncoding;
use tracing::{info, warn};

use crate::config::SourceChain;
use crate::state::Cursor;
use crate::store::{SignerSig, Store, SubmissionRecord};

/// One entry from `getSignaturesForAddress`.
type SignatureEntry = solana_client::rpc_response::RpcConfirmedTransactionStatusWithSignature;

/// Pagination depth for [`Scanner::collect_since_cursor`]. 100 pages × the
/// default 100-signature batch = 10k transactions of backlog before an operator
/// has to intervene.
const MAX_PAGES: usize = 100;

/// What to do after reading one page of signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageAction {
    /// The page reached the cursor (or the start of history): the collected range
    /// is contiguous and safe to process.
    Complete,
    /// The page was full, so older signatures above the cursor remain unread.
    KeepWalking,
    /// Full pages all the way to [`MAX_PAGES`] — the backlog is deeper than we
    /// will walk. Fail rather than process a range with a hole under it.
    TooDeep,
}

/// The pagination rule, extracted so the thing that actually went wrong is
/// testable without an RPC.
///
/// The bug this encodes: a page that comes back FULL is the RPC saying "I hit
/// `limit` before I hit `until`" — i.e. there is more history between this page
/// and the cursor. Treating a full page as the end of the range is what silently
/// skipped events.
fn next_page_action(
    page_len: usize,
    max_batch: usize,
    has_cursor: bool,
    page_index: usize,
) -> PageAction {
    if page_len < max_batch {
        return PageAction::Complete; // reached `until`, or ran out of history
    }
    if !has_cursor {
        return PageAction::Complete; // first run: start at the tip by design
    }
    if page_index + 1 >= MAX_PAGES {
        return PageAction::TooDeep;
    }
    PageAction::KeepWalking
}

/// Recompute a submissionId from a decoded event, exactly as the program did.
///
/// This is THE check: we sign the id we derived, never the one we were handed.
fn recompute(sent: &Sent, bridge_domain: &[u8; 32]) -> [u8; 32] {
    match sent.auto.as_ref() {
        None => submission_id(
            bridge_domain,
            &sent.debridge_id,
            &amount_word(sent.amount as u128),
            sent.chain_id_from,
            sent.chain_id_to,
            sent.nonce,
            &sent.receiver,
        ),
        Some(auto) => submission_id_with_auto(
            bridge_domain,
            &sent.debridge_id,
            &amount_word(sent.amount as u128),
            sent.chain_id_from,
            sent.chain_id_to,
            sent.nonce,
            &sent.receiver,
            auto,
        ),
    }
}

/// 65-byte r||s||v EIP-191 signature over `id`, matching what the EVM validator
/// produces and what `Gate._verifySignatures` accepts.
fn sign(secret: &libsecp256k1::SecretKey, id: &[u8; 32]) -> String {
    let digest = bridge_solana::verify::eth_signed_digest(id);
    let (sig, recid) = libsecp256k1::sign(&libsecp256k1::Message::parse(&digest), secret);
    let mut out = sig.serialize().to_vec();
    out.push(recid.serialize() + 27);
    format!("0x{}", hex::encode(out))
}

/// The signer's EVM address — `keccak(uncompressed_pubkey[1..])[12..]`.
fn evm_address(secret: &libsecp256k1::SecretKey) -> String {
    let public = libsecp256k1::PublicKey::from_secret_key(secret);
    let hash = bridge_solana::hash::keccak(&public.serialize()[1..]);
    format!("0x{}", hex::encode(&hash[12..]))
}

pub struct Scanner {
    rpc: RpcClient,
    program_id: Pubkey,
    /// Deployment generation, read from the gate's `["config"]` PDA at startup
    /// rather than configured — same reasoning as the EVM validator. Signing
    /// under a stale domain would produce ids the gate never derives.
    bridge_domain: [u8; 32],
    cfg: SourceChain,
    secret: libsecp256k1::SecretKey,
    signer_address: String,
    store: Store,
    cursor: Cursor,
}

impl Scanner {
    pub fn new(
        cfg: SourceChain,
        secret_key: [u8; 32],
        store: Store,
    ) -> anyhow::Result<Self> {
        let commitment = match cfg.commitment.as_str() {
            "finalized" => CommitmentConfig::finalized(),
            "confirmed" => CommitmentConfig::confirmed(),
            _ => CommitmentConfig::processed(),
        };
        let secret = libsecp256k1::SecretKey::parse(&secret_key)
            .map_err(|_| anyhow::anyhow!("signer key is not a valid secp256k1 scalar"))?;
        let signer_address = evm_address(&secret);
        let cursor = Cursor::load_or_init(&cfg.state_file)?;
        Ok(Scanner {
            rpc: RpcClient::new_with_commitment(cfg.rpc.clone(), commitment),
            program_id: Pubkey::from_str(&cfg.program_id)
                .map_err(|_| anyhow::anyhow!("program_id is not a valid pubkey"))?,
            bridge_domain: [0u8; 32],
            cfg,
            secret,
            signer_address,
            store,
            cursor,
        })
    }

    pub fn signer_address(&self) -> &str {
        &self.signer_address
    }

    /// Read `bridge_domain` out of the gate's `["config"]` PDA.
    ///
    /// Borsh lays `Config` out in declaration order with no header, so the field
    /// sits at a fixed offset: `owner(32) | bridge_domain(32) | guardian(32) | …`.
    /// Slicing rather than deserializing keeps this crate free of a dependency on
    /// the program crate — at the cost that REORDERING `Config`'s first fields
    /// silently changes what is read here. The owner check below is the guard
    /// that at least proves we are reading the gate's own account.
    async fn load_bridge_domain(&mut self) -> anyhow::Result<()> {
        let (config_pda, _) = Pubkey::find_program_address(&[b"config"], &self.program_id);
        let account = self
            .rpc
            .get_account(&config_pda)
            .await
            .with_context(|| format!("reading gate config PDA {config_pda}"))?;
        anyhow::ensure!(
            account.owner == self.program_id,
            "config PDA {config_pda} is owned by {}, not the gate program",
            account.owner
        );
        let domain: [u8; 32] = account
            .data
            .get(32..64)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| anyhow::anyhow!("config account is too short to hold a bridge_domain"))?;
        anyhow::ensure!(
            domain != [0u8; 32],
            "gate reports a zero bridge_domain — it predates the deployment-domain fix and \
             its attestations would be replayable across deployments"
        );
        self.bridge_domain = domain;
        Ok(())
    }

    /// Poll forever. Transient RPC failures back off and retry rather than kill
    /// the loop; a batch that fails to store leaves the cursor put so the same
    /// range is re-scanned (the store's upsert is idempotent).
    pub async fn run(mut self) -> anyhow::Result<()> {
        let retry = Duration::from_millis(self.cfg.poll_interval_ms.max(500));

        // Before signing anything, learn which deployment generation this gate
        // belongs to. Retried rather than fatal so a cold RPC doesn't kill the
        // process, but the loop below is never entered with a zero domain.
        while let Err(e) = self.load_bridge_domain().await {
            warn!(error = %e, program = %self.program_id, "reading the gate's bridge_domain failed; retrying");
            tokio::time::sleep(retry).await;
        }

        info!(
            validator = %self.signer_address,
            program = %self.program_id,
            bridge_domain = %hex::encode(self.bridge_domain),
            commitment = %self.cfg.commitment,
            resume_after = ?self.cursor.last_signature,
            "solana source scanner started"
        );

        loop {
            match self.tick().await {
                Ok(n) if n > 0 => info!(processed = n, "handled Sent events"),
                Ok(_) => {}
                Err(e) => warn!(error = %e, "scan tick failed; retrying"),
            }
            tokio::time::sleep(retry).await;
        }
    }

    /// Every signature newer than the cursor, oldest-first.
    ///
    /// **This has to paginate.** `getSignaturesForAddress` walks BACKWARDS from
    /// the tip (or from `before`) and stops at `until` *or* at `limit`, whichever
    /// comes first — so a single call on a backlog larger than `limit` returns the
    /// NEWEST `limit` signatures and silently omits the older ones sitting right
    /// above the cursor. Processing that page and advancing the cursor to its
    /// newest entry would skip the omitted range **permanently**: those `Sent`
    /// events would never be signed, and the transfers behind them would never
    /// reach quorum. That is H-3's "cursor advanced past unhandled logs", one
    /// layer down.
    ///
    /// So we walk back page by page until a page reaches the cursor, and only
    /// then hand the caller a contiguous range. If the backlog is deeper than
    /// `max_batch * MAX_PAGES` we return an error and leave the cursor put: the
    /// next tick retries the same range. Falling behind is recoverable; a hole is
    /// not.
    async fn collect_since_cursor(&self) -> anyhow::Result<Vec<SignatureEntry>> {
        let until = self
            .cursor
            .last_signature
            .as_deref()
            .and_then(|s| Signature::from_str(s).ok());

        let mut newest_first: Vec<SignatureEntry> = Vec::new();
        let mut before: Option<Signature> = None;

        for page in 0..MAX_PAGES {
            let sigs = self
                .rpc
                .get_signatures_for_address_with_config(
                    &self.program_id,
                    GetConfirmedSignaturesForAddress2Config {
                        before,
                        until,
                        limit: Some(self.cfg.max_batch),
                        commitment: Some(self.rpc.commitment()),
                    },
                )
                .await?;

            let action = next_page_action(sigs.len(), self.cfg.max_batch, until.is_some(), page);
            let oldest = sigs.last().map(|s| s.signature.clone());
            newest_first.extend(sigs);

            match action {
                PageAction::Complete => {
                    if until.is_none() && page == 0 && !newest_first.is_empty() {
                        info!("no cursor — starting from the current tip, not replaying history");
                    }
                    return Ok(newest_first.into_iter().rev().collect());
                }
                PageAction::TooDeep => anyhow::bail!(
                    "backlog exceeds {} signatures without reaching the cursor; refusing to \
                     advance past unscanned history — raise max_batch or investigate the stall",
                    MAX_PAGES * self.cfg.max_batch
                ),
                PageAction::KeepWalking => {}
            }

            before = match oldest.as_deref().and_then(|s| Signature::from_str(s).ok()) {
                Some(s) => Some(s),
                None => anyhow::bail!("RPC returned an unparseable signature while paginating"),
            };
        }
        unreachable!("the loop returns or bails on its last iteration")
    }

    async fn tick(&mut self) -> anyhow::Result<usize> {
        // Oldest-first, so the nonce sequence and the cursor advance monotonically.
        let entries = self.collect_since_cursor().await?;
        if entries.is_empty() {
            return Ok(0);
        }

        let mut handled = 0usize;
        for entry in &entries {
            if entry.err.is_some() {
                // A failed tx emitted no committed event, but it still counts as
                // scanned — the cursor must move past it or the next tick re-reads
                // the same range forever.
                self.cursor.last_signature = Some(entry.signature.clone());
                self.cursor.save(&self.cfg.state_file)?;
                continue;
            }
            let signature = Signature::from_str(&entry.signature)?;
            let tx = self
                .rpc
                .get_transaction_with_config(
                    &signature,
                    solana_client::rpc_config::RpcTransactionConfig {
                        encoding: Some(UiTransactionEncoding::Json),
                        commitment: Some(self.rpc.commitment()),
                        max_supported_transaction_version: Some(0),
                    },
                )
                .await?;

            let logs = tx
                .transaction
                .meta
                .as_ref()
                .and_then(|m| Option::<Vec<String>>::from(m.log_messages.clone()))
                .unwrap_or_default();

            // ATTRIBUTION FIRST. A transaction's logs are the concatenation of
            // every program that ran in it, and `getSignaturesForAddress` returns
            // transactions that merely MENTION the gate. Parsing all of them would
            // let any program in the transaction dictate what this validator
            // signs. Keep only the lines the gate itself emitted.
            let gate = self.program_id.to_string();
            for line in gate_program_data_lines(&logs, &gate) {
                match parse_sent_event_line(line) {
                    None => continue, // not our event
                    // A tagged-but-malformed payload is a fault, not noise: surface
                    // it and leave the cursor put rather than silently skipping a
                    // transfer (the H3 posture).
                    Some(Err(e)) => {
                        anyhow::bail!("malformed BRIDGE_SENT in tx {}: {e}", entry.signature)
                    }
                    Some(Ok(event)) => {
                        let sent = event.to_sent()?;
                        if self.handle(&event, &sent, &entry.signature).await? {
                            handled += 1;
                        }
                    }
                }
            }

            // Advance only after the whole transaction is durably handled.
            self.cursor.last_signature = Some(entry.signature.clone());
            self.cursor.save(&self.cfg.state_file)?;
        }
        Ok(handled)
    }

    /// Verify and sign one event. `Ok(false)` means the event was rejected as
    /// unauthentic and skipped; `Ok(true)` means it was signed and stored.
    async fn handle(&self, event: &SentEvent, sent: &Sent, tx: &str) -> anyhow::Result<bool> {
        // Never sign an id we cannot reproduce ourselves.
        let computed = recompute(sent, &self.bridge_domain);
        if computed != sent.submission_id {
            anyhow::bail!(
                "submissionId MISMATCH in tx {tx}: emitted {} computed {} — refusing to sign",
                hex::encode(sent.submission_id),
                hex::encode(computed)
            );
        }
        // The gate binds its own chain id into the hash; if it disagrees with our
        // config we are pointed at the wrong program or the wrong cluster.
        if sent.chain_id_from != self.cfg.chain_id {
            anyhow::bail!(
                "event chain_id_from {} != configured {} — refusing to sign",
                sent.chain_id_from,
                self.cfg.chain_id
            );
        }

        // THE origin proof. Recomputing the id proves only that whoever wrote the
        // log hashed their own fields correctly — an attacker does that trivially.
        // The gate's `["sent", submissionId]` PDA is program state only
        // `process_send` can write, so it is the thing that actually distinguishes
        // "the gate locked these funds" from "someone printed a convincing line".
        if !self.origin_proof_holds(event, sent, tx).await? {
            return Ok(false);
        }

        let record = SubmissionRecord {
            submission_id: format!("0x{}", hex::encode(sent.submission_id)),
            // The SAME domain this scanner recomputed the id under, so the store
            // re-derives the identical id. Reading it from `self` (which loaded it
            // from the chain) rather than from config means a relayer can never
            // attest under a domain the gate does not actually carry.
            bridge_domain: format!("0x{}", hex::encode(self.bridge_domain)),
            debridge_id: format!("0x{}", hex::encode(sent.debridge_id)),
            amount: sent.amount.to_string(),
            chain_id_from: sent.chain_id_from,
            chain_id_to: sent.chain_id_to,
            nonce: sent.nonce,
            receiver: format!("0x{}", hex::encode(&sent.receiver)),
            // Solana auto-params ride in the id, not as EVM-encoded bytes; the
            // store treats an empty string as "no payload".
            auto_params: "0x".to_string(),
            native_sender: format!("0x{}", hex::encode(&sent.native_sender)),
            // `token` is the EVM-side ERC-20 for the refund relayer. A Solana
            // transfer's asset is the SPL mint, which does not hash to the same
            // debridgeId formula, so it is deliberately left empty rather than
            // filled with something the store would reject.
            token: String::new(),
            signatures: vec![SignerSig {
                signer: self.signer_address.clone(),
                signature: sign(&self.secret, &sent.submission_id),
            }],
            cancel_signatures: vec![],
            refund_signatures: vec![],
        };

        self.store.upsert(&record).await?;
        info!(
            submission_id = %record.submission_id,
            nonce = sent.nonce,
            chain_to = sent.chain_id_to,
            "SIGNED and stored"
        );
        Ok(true)
    }

    /// Read the gate's `["sent", submissionId]` record and check it corroborates
    /// the event.
    ///
    /// The two failure modes are deliberately NOT treated alike:
    ///
    ///   * **unauthentic** (no record, foreign-owned, disagrees) — this is not our
    ///     event. Warn loudly and skip. It must not abort the tick: a forged event
    ///     is cheap to emit, so failing the batch would let anyone wedge the
    ///     scanner permanently by spamming them — turning a foiled theft into a
    ///     denial of service on every real transfer behind it.
    ///   * **unreadable** (RPC error) — we do not KNOW. Propagate, so the cursor
    ///     stays put and the tick retries. Never sign on a failed lookup.
    async fn origin_proof_holds(
        &self,
        event: &SentEvent,
        sent: &Sent,
        tx: &str,
    ) -> anyhow::Result<bool> {
        let (pda, _bump) = Pubkey::find_program_address(
            &[b"sent", &sent.submission_id],
            &self.program_id,
        );
        let account = self
            .rpc
            .get_account_with_commitment(&pda, self.rpc.commitment())
            .await
            .with_context(|| format!("reading [\"sent\"] PDA {pda} for tx {tx}"))?
            .value;

        let view = account
            .as_ref()
            .map(|a| (a.owner == self.program_id, a.data.as_slice()));

        match verify_sent_record(view, event) {
            Ok(_) => Ok(true),
            Err(e) => {
                warn!(
                    tx,
                    submission_id = %hex::encode(sent.submission_id),
                    sent_pda = %pda,
                    amount = sent.amount,
                    chain_to = sent.chain_id_to,
                    error = %e,
                    "REJECTED unauthentic BRIDGE_SENT — no matching on-chain origin \
                     proof; refusing to sign (possible forged event)"
                );
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// Any non-zero domain: these tests assert recompute is SELF-consistent and
    /// detects tampering, neither of which depends on the specific value.
    const TEST_DOMAIN: [u8; 32] = [0xD0; 32];

    use super::*;
    use bridge_solana::relayer::{sent_event_to_program_data_line, SentEvent};

    fn sample() -> Sent {
        let mut s = Sent {
            submission_id: [0u8; 32],
            debridge_id: [0x22; 32],
            amount: 42_000,
            chain_id_from: 7565164,
            chain_id_to: 1337,
            receiver: vec![0xEE; 20],
            nonce: 7,
            native_sender: vec![0x33; 32],
            auto: None,
        };
        s.submission_id = recompute(&s, &TEST_DOMAIN);
        s
    }

    /// The scanner must derive the same id the program did — otherwise every
    /// signature it produces is for a submission no gate will ever recognise.
    #[test]
    fn recompute_round_trips_through_the_real_log_framing() {
        let sent = sample();
        let line = sent_event_to_program_data_line(&SentEvent::from_sent(&sent, [0x55; 32]));
        let parsed = parse_sent_event_line(&line)
            .expect("our line")
            .expect("decodes")
            .to_sent()
            .expect("converts");
        assert_eq!(recompute(&parsed, &TEST_DOMAIN), sent.submission_id, "id must survive the round trip");
    }

    /// C-1, at the layer the scanner actually reads.
    ///
    /// `getSignaturesForAddress(gate)` returns transactions that merely MENTION
    /// the gate, and their logs carry every program's output. The scanner used to
    /// parse all of them, so a forged `BRIDGE_SENT` from an attacker's program was
    /// indistinguishable from a real one — it recomputes, its `chain_id_from` is
    /// whatever the attacker wrote, and the validator signed it. That signature,
    /// times threshold, releases real liquidity on the EVM destination.
    ///
    /// The scanner now selects lines by emitting program before parsing.
    #[test]
    fn a_forged_event_from_another_program_never_reaches_the_parser() {
        const GATE: &str = "GateProg11111111111111111111111111111111111";
        const EVIL: &str = "EvilProg11111111111111111111111111111111111";

        // The attacker's payload: a real corridor, an enormous amount, their own
        // receiver — and a correctly recomputed id, because they hash their own
        // fields honestly. Nothing downstream can tell it apart.
        let mut forged = sample();
        forged.amount = 1_000_000_000_000;
        forged.receiver = vec![0xAA; 20]; // the attacker's EVM address
        forged.submission_id = recompute(&forged, &TEST_DOMAIN);

        let line = sent_event_to_program_data_line(&SentEvent::from_sent(&forged, [0x55; 32]));
        let logs: Vec<String> = vec![
            format!("Program {EVIL} invoke [1]"),
            line.clone(),
            format!("Program {EVIL} success"),
        ];

        // Pre-fix behaviour: the line parses, and recompute agrees with it.
        let decoded = parse_sent_event_line(&line).unwrap().unwrap().to_sent().unwrap();
        assert_eq!(recompute(&decoded, &TEST_DOMAIN), decoded.submission_id, "the forgery is self-consistent");

        // Post-fix: it is never attributed to the gate, so it is never parsed.
        assert!(
            gate_program_data_lines(&logs, GATE).is_empty(),
            "a foreign program's forged Sent must never reach the signing path"
        );
    }

    /// A tampered event must not reproduce its claimed id — this is the check that
    /// stops a lying RPC getting a signature over params nobody sent.
    #[test]
    fn a_tampered_event_fails_recomputation() {
        let mut sent = sample();
        sent.amount += 1; // the classic: inflate the payout
        assert_ne!(recompute(&sent, &TEST_DOMAIN), sent.submission_id, "tampering must be detectable");
    }

    /// THE cursor bug, stated as a rule.
    ///
    /// `getSignaturesForAddress` walks backwards from the tip and stops at
    /// `until` OR `limit`. A full page therefore means "I hit the limit first" —
    /// there is unread history between this page and the cursor. The scanner used
    /// to process that page and set the cursor to its newest entry, which skipped
    /// the unread range permanently: those `Sent` events were never signed, so
    /// their transfers could never reach quorum.
    #[test]
    fn a_full_page_means_there_is_more_history_below_it() {
        // Backlog deeper than one page, cursor present -> must keep walking.
        assert_eq!(
            next_page_action(100, 100, true, 0),
            PageAction::KeepWalking,
            "a full page must never be treated as the end of the range"
        );
        // Short page -> the walk reached the cursor; the range is contiguous.
        assert_eq!(next_page_action(37, 100, true, 0), PageAction::Complete);
        assert_eq!(next_page_action(0, 100, true, 3), PageAction::Complete);
    }

    /// A first run has no cursor, so there is no range to be contiguous WITH:
    /// starting at the tip is deliberate, not a skip.
    #[test]
    fn the_first_run_starts_at_the_tip() {
        assert_eq!(next_page_action(100, 100, false, 0), PageAction::Complete);
    }

    /// Falling too far behind must fail loudly and leave the cursor put. Silently
    /// processing a range with a hole under it is the failure mode we are fixing;
    /// re-reading the same range next tick is merely slow.
    #[test]
    fn an_unwalkable_backlog_fails_instead_of_skipping() {
        assert_eq!(next_page_action(100, 100, true, MAX_PAGES - 1), PageAction::TooDeep);
        // Still walkable one page earlier.
        assert_eq!(next_page_action(100, 100, true, MAX_PAGES - 2), PageAction::KeepWalking);
    }

    /// The signature must recover to the address we claim, or the store rejects it.
    #[test]
    fn signature_recovers_to_the_claimed_signer() {
        let secret = libsecp256k1::SecretKey::parse(&[7u8; 32]).unwrap();
        let id = [0x11u8; 32];
        let sig_hex = sign(&secret, &id);
        let raw = hex::decode(sig_hex.trim_start_matches("0x")).unwrap();

        let digest = bridge_solana::verify::eth_signed_digest(&id);
        let recovered = libsecp256k1::recover(
            &libsecp256k1::Message::parse(&digest),
            &libsecp256k1::Signature::parse_standard_slice(&raw[..64]).unwrap(),
            &libsecp256k1::RecoveryId::parse(raw[64] - 27).unwrap(),
        )
        .unwrap();
        let hash = bridge_solana::hash::keccak(&recovered.serialize()[1..]);
        assert_eq!(format!("0x{}", hex::encode(&hash[12..])), evm_address(&secret));
    }
}
